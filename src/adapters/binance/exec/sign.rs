//! HMAC-SHA256 signing gate (every private call). Params -> exact signed bytes (REST = URL,
//! WS = JSON). Never unsigned on wire.

use std::fmt;

use ring::hmac;

use crate::secrets::Secret;
use crate::time::{DurationUs, TsUs};

const TIMESTAMP_PARAM: &str = "timestamp";
const SIGNATURE_PARAM: &str = "signature";

const RECV_WINDOW_PARAM: &str = "recvWindow";
const RECV_WINDOW_MAX_MILLIS: u32 = 60_000;

const HEX_DIGITS: [u8; 16] = *b"0123456789abcdef";

/// Binance timestamp tolerance. Widen -> replay exposure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecvWindow(u32);

impl RecvWindow {
    pub const DEFAULT: RecvWindow = RecvWindow(5_000);

    /// # Errors
    /// Out of range 1..=60000.
    pub fn from_millis(millis: u32) -> Result<Self, SignError> {
        if millis == 0 || millis > RECV_WINDOW_MAX_MILLIS {
            return Err(SignError::RecvWindowOutOfRange { millis });
        }
        Ok(Self(millis))
    }

    pub const fn millis(self) -> u32 {
        self.0
    }
}

/// Binance timestamp ms since epoch. Minted by ClockOffset::stamp only (never uncorrected).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestStamp(i64);

impl RequestStamp {
    pub const fn millis(self) -> i64 {
        self.0
    }
}

/// server_time - local_time (learned from GET /api/v3/time). Slow host -> recvWindow rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockOffset {
    correction: DurationUs,
}

impl ClockOffset {
    pub const NONE: ClockOffset = ClockOffset {
        correction: DurationUs::ZERO,
    };

    /// observed_at = response arrival. Round-trip: safe at receipt (lags), unsafe at send.
    pub fn learn(server_time: TsUs, observed_at: TsUs) -> Self {
        Self {
            correction: server_time.diff(observed_at),
        }
    }

    pub const fn correction(self) -> DurationUs {
        self.correction
    }

    /// Parameter not clock -> replay identical. Floors to safe side.
    pub fn stamp(self, local_now: TsUs) -> RequestStamp {
        RequestStamp((local_now + self.correction).micros().div_euclid(1_000))
    }
}

/// Params unsigned alone -> only RequestSigner::sign yields sendable.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RequestParams {
    entries: Vec<(&'static str, String)>,
}

impl RequestParams {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace if exists (venue rejects duplicates).
    #[must_use]
    pub fn set(mut self, name: &'static str, value: impl Into<String>) -> Self {
        let value = value.into();
        match self.entries.iter_mut().find(|(held, _)| *held == name) {
            Some(entry) => entry.1 = value,
            None => self.entries.push((name, value)),
        }
        self
    }

    #[must_use]
    pub fn set_recv_window(self, window: RecvWindow) -> Self {
        self.set(RECV_WINDOW_PARAM, window.millis().to_string())
    }
}

/// HMAC-SHA256 lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Signature(String);

impl Signature {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Params + signature = only sendable private request form.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedRequest {
    signed_params: Vec<(&'static str, String)>,
    signature: Signature,
    query: String,
}

impl SignedRequest {
    /// Exact bytes + sig VERBATIM to URL; never via reqwest.query().
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Sorted, sig excluded. WS JSON adds sig key. Venue verifies sorted payload.
    pub fn signed_params(&self) -> &[(&'static str, String)] {
        &self.signed_params
    }

    pub fn signature(&self) -> &Signature {
        &self.signature
    }
}

/// Derived Debug leaks: query replayable. WS apiKey is param (plaintext). Names only.
impl fmt::Debug for SignedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.signed_params.iter().map(|(name, _)| *name).collect();
        formatter
            .debug_struct("SignedRequest")
            .field("signed_params", &names)
            .field("signature", &"<redacted>")
            .field("query", &"<redacted>")
            .finish()
    }
}

pub struct RequestSigner {
    key: hmac::Key,
}

impl RequestSigner {
    pub fn new(api_secret: &Secret) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, api_secret.expose_bytes()),
        }
    }

    /// The raw HMAC over a payload exactly as given. [`RequestSigner::sign`] is what a request goes
    /// through, because that is where the timestamp and recvWindow gate lives; this one is public so
    /// the venue's own documented payload/signature pair can be pinned verbatim, which is the only
    /// check on this code that does not grade itself.
    pub fn sign_payload(&self, payload: &str) -> Signature {
        let tag = hmac::sign(&self.key, payload.as_bytes());
        Signature(hex_lower(tag.as_ref()))
    }

    /// Sort by name, stamp, sign. Caller timestamp/sig dropped.
    ///
    /// # Errors
    /// Not URL safe (char illegal unescaped -> divergence).
    pub fn sign(
        &self,
        params: RequestParams,
        stamp: RequestStamp,
    ) -> Result<SignedRequest, SignError> {
        let mut signed_params = params.entries;
        signed_params.retain(|(name, _)| *name != TIMESTAMP_PARAM && *name != SIGNATURE_PARAM);
        signed_params.push((TIMESTAMP_PARAM, stamp.millis().to_string()));
        signed_params.sort_by_key(|(name, _)| *name);

        for (name, value) in &signed_params {
            check_query_safe(name, value)?;
        }

        let payload = join_params(&signed_params);
        let signature = self.sign_payload(&payload);
        let query = format!("{payload}&{SIGNATURE_PARAM}={}", signature.as_str());
        Ok(SignedRequest {
            signed_params,
            signature,
            query,
        })
    }
}

/// Hand-written (prevent ring::Key rendering from leaking key material).
impl fmt::Debug for RequestSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequestSigner(<hmac-sha256>)")
    }
}

#[derive(thiserror::Error, Debug)]
pub enum SignError {
    #[error(
        "param {name} holds a character at byte {position} of `name=value` that a url query cannot carry unescaped — binance private params are unreserved characters plus `:` and `/`"
    )]
    NotUrlSafe { name: String, position: usize },
    #[error("recvWindow {millis}ms is out of range — binance accepts 1..=60000")]
    RecvWindowOutOfRange { millis: u32 },
}

/// Binance params legal unencoded (symbols/enums/decimals/charset). Refuse others.
fn check_query_safe(name: &str, value: &str) -> Result<(), SignError> {
    let value_offset = name.len() + 1;
    let offender = name
        .char_indices()
        .chain(
            value
                .char_indices()
                .map(|(position, character)| (position + value_offset, character)),
        )
        .find(|(_, character)| !is_query_safe(*character));
    match offender {
        Some((position, _)) => Err(SignError::NotUrlSafe {
            name: name.to_owned(),
            position,
        }),
        None => Ok(()),
    }
}

fn is_query_safe(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '~' | ':' | '/')
}

fn join_params(entries: &[(&'static str, String)]) -> String {
    entries
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        hex.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    hex
}
