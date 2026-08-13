//! Startup gate: prove the wallet can trade before any threads start. Not a formality on Polymarket.
//! Region geoblocks make accounts close-only (not rejected), so unchecked runs appear as strategies
//! that never fire. L2 credentials don't exist until the wallet key mints them, so they can't be
//! configured in advance — only checked at boot.

use std::path::Path;
use std::time::Duration;

use crate::secrets::{EnvFile, Secret, SecretError};
use crate::time::DurationUs;
use crate::{info, warn};

use super::codec::{
    ApiKeyPayload, EncodedRequest, PROTOCOL_VERSION, closed_only_status, collateral_balance,
    create_api_key, decode_closed_only, decode_protocol_version, derive_api_key, open_orders_page,
    protocol_version, server_time,
};
use super::handle::WalletIdentity;
use super::probe::Geoblock;
use super::rest::{CLOB_BASE, ClobHttp, ClobHttpError, ClobResponse, GEOBLOCK_URL};
use super::sign::address::{Address, AddressError};
use super::sign::key::{KeyError, SigningKey};
use super::sign::l2::{ApiCredentials, L2Error, RequestSigner};
use super::sign::order::SignatureType;
use super::{POLYMARKET_CREDENTIAL_VARIABLES, SIGNATURE_TYPE_VARIABLE, WALLET_ADDRESS_VARIABLE};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// Gate result: credentials, key, and identity the adapter uses, avoiding re-derivation.
pub struct PolymarketPreflight {
    // Minted by L1 at boot, never persisted.
    pub credentials: ApiCredentials,
    pub key: SigningKey,
    pub wallet: WalletIdentity,
    // Applied to local clock to stamp POLY_TIMESTAMP in venue time.
    pub venue_clock_offset: DurationUs,
}

/// Prove this wallet may trade before any thread spawns.
///
/// Placement is the only ground truth for a region restriction, so a geoblocked host is REPORTED
/// and allowed to continue; the account's own close-only flag is a refusal, because the venue has
/// already decided.
///
/// # Errors
/// [`PolymarketPreflightError`] for absent or contradictory wallet configuration, a venue that
/// cannot be reached or speaks another protocol version, credentials it refuses to mint, and an
/// account it will not let open a position.
pub async fn preflight_polymarket() -> Result<PolymarketPreflight, PolymarketPreflightError> {
    let secrets = EnvFile::load(Path::new(EnvFile::DEFAULT_PATH))?;
    let key =
        SigningKey::from_secret(&secrets.resolve(POLYMARKET_CREDENTIAL_VARIABLES.api_secret_env)?)?;
    let wallet = wallet_identity(&secrets, &key)?;

    let clob = GateReads::new()?;
    let body = clob.public_text(&protocol_version()).await?;
    let version = decode_protocol_version(&body);
    if version != Some(PROTOCOL_VERSION) {
        return Err(PolymarketPreflightError::ProtocolVersion {
            // Unreadable answers report the body itself: an operator cannot act on "not 2".
            found: version.map_or(body, |version| version.to_string()).into(),
            expected: PROTOCOL_VERSION,
        });
    }
    let server_time_secs = clob.server_time().await?;
    let venue_clock_offset = DurationUs::from_secs(server_time_secs - local_unix_secs());
    info!(
        "polymarket venue clock offset {}us, protocol v{PROTOCOL_VERSION}",
        venue_clock_offset.micros()
    );
    report_geoblock(clob.geoblock().await?);

    let credentials = clob.api_credentials(&key, server_time_secs).await?;
    let signer = RequestSigner::new(&credentials, wallet.signer)
        .map_err(|source| PolymarketPreflightError::Signer { source })?;
    if clob.is_closed_only(&signer, server_time_secs).await? {
        return Err(PolymarketPreflightError::ClosedOnly);
    }
    info!(
        "polymarket account ok: may open positions, maker {} signing as {} (signatureType {})",
        wallet.maker.to_checksum_hex(),
        wallet.signer.to_checksum_hex(),
        wallet.signature_type.code()
    );
    clob.report_account(&signer, wallet, server_time_secs).await;

    Ok(PolymarketPreflight {
        credentials,
        key,
        wallet,
        venue_clock_offset,
    })
}

#[derive(thiserror::Error, Debug)]
pub enum PolymarketPreflightError {
    #[error(
        "polymarket credentials could not be loaded — execution needs a wallet key it can sign with"
    )]
    Credentials(#[from] SecretError),
    #[error("{variable} holds bytes that are not utf-8")]
    NotUtf8 { variable: &'static str },
    #[error("the polymarket wallet private key could not be read")]
    Key(#[from] KeyError),
    #[error(
        "{} names {configured} but the private key derives {derived} — one of the two is another account's",
        POLYMARKET_CREDENTIAL_VARIABLES.api_key_env
    )]
    SignerMismatch {
        configured: Box<str>,
        derived: Box<str>,
    },
    #[error(
        "{WALLET_ADDRESS_VARIABLE} = {value:?} is not an ethereum address — run tools/poly-probe to discover this account's"
    )]
    WalletAddress {
        value: Box<str>,
        #[source]
        source: AddressError,
    },
    #[error(
        "{SIGNATURE_TYPE_VARIABLE} = {value:?}, expected 0 (eoa), 1 (proxy), 2 (gnosis safe) or 3 (deposit wallet) — run tools/poly-probe to discover this account's"
    )]
    WalletSignatureType { value: Box<str> },
    #[error(
        "the venue could not be reached — refusing to start without proof the wallet can trade"
    )]
    Http(#[from] ClobHttpError),
    #[error("{url} answered http {status}: {body}")]
    Status {
        url: String,
        status: u16,
        body: String,
    },
    #[error("{url} answered a body this engine cannot read: {body}")]
    UnexpectedBody { url: String, body: String },
    #[error(
        "the polymarket clob speaks protocol {found}, and every payload this engine signs is shaped for {expected}"
    )]
    ProtocolVersion { found: Box<str>, expected: u32 },
    #[error(
        "the venue minted no l2 credentials for this wallet — derive said: {derive}; create said: {create}"
    )]
    NoCredentials { derive: String, create: String },
    #[error("the minted l2 credentials cannot be used to sign")]
    Signer {
        #[source]
        source: L2Error,
    },
    #[error(
        "this polymarket account is closed-only and may only reduce positions — a market maker on it would quote one side into rejection"
    )]
    ClosedOnly,
}

/// The wallet type and account address are discovered, not configured, so the engine reads back what
/// `tools/poly-probe` wrote rather than guessing at them.
fn wallet_identity(
    secrets: &EnvFile,
    key: &SigningKey,
) -> Result<WalletIdentity, PolymarketPreflightError> {
    let signer = key.address();
    let configured = read_plain(secrets, POLYMARKET_CREDENTIAL_VARIABLES.api_key_env)?;
    if !configured.eq_ignore_ascii_case(&signer.to_checksum_hex()) {
        return Err(PolymarketPreflightError::SignerMismatch {
            configured: configured.into_boxed_str(),
            derived: signer.to_checksum_hex().into_boxed_str(),
        });
    }
    let maker_text = read_plain(secrets, WALLET_ADDRESS_VARIABLE)?;
    let maker =
        Address::parse(&maker_text).map_err(|source| PolymarketPreflightError::WalletAddress {
            value: maker_text.into_boxed_str(),
            source,
        })?;
    let code_text = read_plain(secrets, SIGNATURE_TYPE_VARIABLE)?;
    let signature_type = code_text
        .trim()
        .parse::<u8>()
        .ok()
        .and_then(|code| SignatureType::from_code(code).ok())
        .ok_or_else(|| PolymarketPreflightError::WalletSignatureType {
            value: code_text.into_boxed_str(),
        })?;
    Ok(WalletIdentity {
        maker,
        signer,
        signature_type,
    })
}

/// Addresses and wallet types are not secrets; they arrive through the secret loader only because
/// that is where `.env` is read.
fn read_plain(
    secrets: &EnvFile,
    variable: &'static str,
) -> Result<String, PolymarketPreflightError> {
    let value = secrets.resolve(variable)?;
    str::from_utf8(value.expose_bytes())
        .map(str::to_owned)
        .map_err(|_| PolymarketPreflightError::NotUtf8 { variable })
}

/// Reported, never refused: the flag is a website signal, and the venue's answer to a placement is
/// the only ground truth about whether this host may trade.
fn report_geoblock(geoblock: Geoblock) {
    if !geoblock.blocked {
        return;
    }
    warn!(
        "polymarket reports this host geoblocked (ip {}, country {}, region {}) — placement may be refused at the venue",
        blank_as_dash(&geoblock.ip),
        blank_as_dash(&geoblock.country),
        blank_as_dash(&geoblock.region)
    );
}

/// The signed reads the gate needs, over the same transport the adapter itself uses.
struct GateReads {
    http: ClobHttp,
}

impl GateReads {
    fn new() -> Result<Self, PolymarketPreflightError> {
        Ok(Self {
            http: ClobHttp::new(CONNECT_TIMEOUT, REQUEST_TIMEOUT)?,
        })
    }

    async fn geoblock(&self) -> Result<Geoblock, PolymarketPreflightError> {
        let response = self.http.send_unsigned(GEOBLOCK_URL).await?;
        decode(GEOBLOCK_URL, &response)
    }

    async fn public_text(
        &self,
        request: &EncodedRequest,
    ) -> Result<String, PolymarketPreflightError> {
        let response = self.http.send_public(request).await?;
        Ok(success_body(&request.path, &response)?.trim().to_owned())
    }

    async fn server_time(&self) -> Result<i64, PolymarketPreflightError> {
        let body = self.public_text(&server_time()).await?;
        body.parse()
            .map_err(|_| PolymarketPreflightError::UnexpectedBody {
                url: format!("{CLOB_BASE}/time"),
                body: body.clone(),
            })
    }

    /// Derive first: it is idempotent for an address that already has credentials, where create
    /// would fail. A fresh wallet has none to derive, so create is the fallback rather than the
    /// first move.
    async fn api_credentials(
        &self,
        key: &SigningKey,
        timestamp_secs: i64,
    ) -> Result<ApiCredentials, PolymarketPreflightError> {
        let derived = self
            .credential_call(key, &derive_api_key(), timestamp_secs)
            .await;
        match derived {
            Ok(credentials) => Ok(credentials),
            Err(derive_failure) => self
                .credential_call(key, &create_api_key(), timestamp_secs)
                .await
                .map_err(|create_failure| PolymarketPreflightError::NoCredentials {
                    derive: derive_failure.to_string(),
                    create: create_failure.to_string(),
                }),
        }
    }

    async fn credential_call(
        &self,
        key: &SigningKey,
        request: &EncodedRequest,
        timestamp_secs: i64,
    ) -> Result<ApiCredentials, PolymarketPreflightError> {
        let response = self
            .http
            .send_wallet_signed(key, request, timestamp_secs)
            .await?;
        let payload: ApiKeyPayload = decode(&request.path, &response)?;
        Ok(ApiCredentials::new(
            payload.api_key,
            Secret::new(&payload.secret),
            Secret::new(&payload.passphrase),
        ))
    }

    /// Fail-closed: a body that does not carry the flag refuses the arm rather than reading as "may
    /// trade". The `#[serde(default)] bool` this replaced let an error envelope or `{}` pass the gate.
    async fn is_closed_only(
        &self,
        signer: &RequestSigner,
        timestamp_secs: i64,
    ) -> Result<bool, PolymarketPreflightError> {
        let request = closed_only_status();
        let response = self
            .http
            .send_signed(signer, &request, timestamp_secs)
            .await?;
        let body = success_body(&request.path, &response)?;
        decode_closed_only(body).ok_or_else(|| PolymarketPreflightError::UnexpectedBody {
            url: request.path.clone(),
            body: response.excerpt(),
        })
    }

    /// What the operator has to see before a live arm: the collateral this wallet type actually
    /// holds, and whether anything is already resting on the credentials.
    async fn report_account(
        &self,
        signer: &RequestSigner,
        wallet: WalletIdentity,
        timestamp_secs: i64,
    ) {
        report_read(
            "collateral",
            self.http
                .send_signed(
                    signer,
                    &collateral_balance(wallet.signature_type),
                    timestamp_secs,
                )
                .await,
        );
        report_read(
            "open orders",
            self.http
                .send_signed(signer, &open_orders_page(None), timestamp_secs)
                .await,
        );
    }
}

fn report_read(what: &str, answer: Result<ClobResponse, ClobHttpError>) {
    match answer {
        Ok(response) if response.is_success() => {
            info!("polymarket {what} before arming: {}", response.excerpt());
        }
        Ok(response) => warn!(
            "polymarket {what} read answered http {} — arming against a number it could not confirm: {}",
            response.status,
            response.excerpt()
        ),
        Err(error) => warn!("polymarket {what} read failed: {error}"),
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    path: &str,
    response: &ClobResponse,
) -> Result<T, PolymarketPreflightError> {
    let body = success_body(path, response)?;
    serde_json::from_str(body).map_err(|_| PolymarketPreflightError::UnexpectedBody {
        url: path.to_owned(),
        body: response.excerpt(),
    })
}

fn success_body<'a>(
    path: &str,
    response: &'a ClobResponse,
) -> Result<&'a str, PolymarketPreflightError> {
    if !response.is_success() {
        return Err(PolymarketPreflightError::Status {
            url: path.to_owned(),
            status: response.status,
            body: response.excerpt(),
        });
    }
    Ok(&response.body)
}

fn blank_as_dash(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

fn local_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}
