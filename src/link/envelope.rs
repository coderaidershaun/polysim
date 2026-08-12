//! Universal envelope, validation order, little-endian primitives. Version skew caught before book misdecode.

use crate::ids::{Price, Qty};
use crate::time::{DurationUs, TsUs};

use super::wire::{WireField, wire_struct};

/// `"PLNK"` — a foreign datagram or a mistyped port is rejected on the first four bytes.
pub const LINK_MAGIC: u32 = 0x504C_4E4B;

/// Bump on ANY layout change (UI_BOOK_LEVELS, Level, etc). Only thing catching cross-commit skew.
pub const LINK_VERSION: u16 = 10;

/// Over-length panics (never truncates); margin real but thin (max 30 chars today).
pub const LINK_NAME_LEN: usize = 32;

/// Below IPv6 1280-byte MTU; no fragmentation.
pub const LINK_MAX_DATAGRAM: usize = 1200;

/// Reserved+zero for MAC retrofit. Link security: private network boundary.
const LINK_MAC_LEN: usize = 32;

pub(super) const ENVELOPE_LEN: usize = 4 + 2 + 2 + 8 + 8 + 8 + 8 + 8 + LINK_MAC_LEN;
pub(super) const OPTIONAL_MANTISSA_LEN: usize = 1 + 8;
pub(super) const OPTIONAL_F64_LEN: usize = 1 + 8;
pub(super) const OPTIONAL_LEVEL_LEN: usize = 1 + 8 + 8;
pub(super) const NAME_LEN: usize = 1 + LINK_NAME_LEN;

const ABSENT: u8 = 0;
const PRESENT: u8 = 1;

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a digest; no strings on wire. Not security (sniffable); separates runs only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkHash(pub u64);

impl LinkHash {
    pub const fn of_name(name: &str) -> Self {
        Self(fold(FNV_OFFSET_BASIS, name.as_bytes()))
    }

    /// Ordered field-name digest; order+count both in digest -> read slot 7 as 7, not 8.
    pub const fn of_fields(names: &[&str]) -> Self {
        let mut hash = FNV_OFFSET_BASIS;
        let mut index = 0;
        while index < names.len() {
            hash = fold(hash, names[index].as_bytes());
            hash = fold(hash, &[0]);
            index += 1;
        }
        Self(hash)
    }
}

const fn fold(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        index += 1;
    }
    hash
}

/// Topic <-> body kind one-to-one; below FIRST_STRATEGY reserved (strategy claim = config error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TopicId(pub u16);

impl TopicId {
    pub const SUBSCRIBE: TopicId = TopicId(0);
    pub const BOOKS: TopicId = TopicId(1);
    pub const EVENTS: TopicId = TopicId(2);
    pub const CATALOG_INSTRUMENTS: TopicId = TopicId(3);
    pub const CATALOG_FEATURES: TopicId = TopicId(4);
    pub const LIFECYCLE: TopicId = TopicId(5);
    pub const FIRST_STRATEGY: TopicId = TopicId(16);

    /// Id of the strategy topic declared at `index`. The numbering lives here because the sites
    /// that resolve a topic name, stamp ids on outbound frames, and size the per-topic sequence
    /// array must agree — a disagreement surfaces as an out-of-bounds index on the link send path
    /// rather than as a startup rejection.
    ///
    /// # Panics
    /// Index past the u16 topic space (a strategy declaring tens of thousands of topics).
    #[inline]
    pub fn strategy(index: usize) -> Self {
        let raw = usize::from(Self::FIRST_STRATEGY.0) + index;
        TopicId(u16::try_from(raw).expect("strategy topic index overflows the u16 topic space"))
    }

    /// Width of the id space once `strategy_topic_count` strategy topics exist — the length an
    /// array indexed by raw topic id must have.
    #[inline]
    pub fn space_len(strategy_topic_count: usize) -> usize {
        usize::from(Self::FIRST_STRATEGY.0) + strategy_topic_count
    }

    #[inline]
    pub const fn is_strategy_topic(self) -> bool {
        self.0 >= Self::FIRST_STRATEGY.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkIdentity {
    pub token_hash: LinkHash,
    pub strategy_hash: LinkHash,
    pub sender_te_hash: LinkHash,
    pub boot_ts_us: TsUs,
}

/// boot_ts_us load-bearing: restart sets seq=0; (sender,topic)-only gate reads as stale until seq passes old HWM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Envelope {
    pub magic: u32,
    pub version: u16,
    pub topic: TopicId,
    pub token_hash: LinkHash,
    pub strategy_hash: LinkHash,
    pub sender_te_hash: LinkHash,
    pub boot_ts_us: TsUs,
    pub seq: u64,
    pub mac: [u8; LINK_MAC_LEN],
}

impl Envelope {
    pub fn new(identity: LinkIdentity, topic: TopicId, seq: u64) -> Self {
        Self {
            magic: LINK_MAGIC,
            version: LINK_VERSION,
            topic,
            token_hash: identity.token_hash,
            strategy_hash: identity.strategy_hash,
            sender_te_hash: identity.sender_te_hash,
            boot_ts_us: identity.boot_ts_us,
            seq,
            mac: [0; LINK_MAC_LEN],
        }
    }
}

wire_struct! {
    Envelope {
        magic,
        version,
        topic,
        token_hash,
        strategy_hash,
        sender_te_hash,
        boot_ts_us,
        seq,
        mac,
    }
}

/// The reserved MAC moves as one block rather than field-by-field: it is the only run of bytes here
/// that carries no structure.
impl WireField for [u8; LINK_MAC_LEN] {
    fn write(&self, writer: &mut ByteWriter<'_>) {
        writer.write_bytes(self);
    }

    fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
        Ok(reader.take())
    }
}

/// schema_hash gates strategy payloads only (engine topics carry no schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameGuard {
    pub token_hash: LinkHash,
    pub strategy_hash: LinkHash,
    pub schema_hash: LinkHash,
}

impl FrameGuard {
    pub(super) fn check(&self, envelope: &Envelope) -> Result<(), LinkDecodeError> {
        if envelope.magic != LINK_MAGIC {
            return Err(LinkDecodeError::MagicMismatch {
                found: envelope.magic,
            });
        }
        if envelope.version != LINK_VERSION {
            return Err(LinkDecodeError::VersionMismatch {
                found: envelope.version,
            });
        }
        if envelope.token_hash != self.token_hash {
            return Err(LinkDecodeError::TokenMismatch {
                found: envelope.token_hash,
                expected: self.token_hash,
            });
        }
        if envelope.strategy_hash != self.strategy_hash {
            return Err(LinkDecodeError::StrategyMismatch {
                found: envelope.strategy_hash,
                expected: self.strategy_hash,
            });
        }
        Ok(())
    }
}

/// Fixed bytes; validated UTF-8 by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireName {
    bytes: [u8; LINK_NAME_LEN],
    len: u8,
}

impl WireName {
    /// # Panics
    /// Over LINK_NAME_LEN bytes (startup code-level fact; truncate -> silent column mislabel).
    pub fn new(name: &str) -> Self {
        assert!(
            name.len() <= LINK_NAME_LEN,
            "link name {name:?} is {} bytes, capacity {LINK_NAME_LEN}",
            name.len()
        );
        let mut bytes = [0; LINK_NAME_LEN];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        Self {
            bytes,
            len: name.len() as u8,
        }
    }

    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.bytes[..self.len as usize])
            .expect("WireName holds validated utf8 by construction")
    }
}

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDecodeError {
    #[error("datagram is {found} bytes, shorter than the {ENVELOPE_LEN}-byte envelope")]
    Truncated { found: usize },
    #[error("magic {found:#010x} is not the link magic {LINK_MAGIC:#010x}")]
    MagicMismatch { found: u32 },
    #[error("wire version {found} is not {LINK_VERSION} — peer built from another commit")]
    VersionMismatch { found: u16 },
    #[error("token hash {found:?} is not the configured {expected:?}")]
    TokenMismatch { found: LinkHash, expected: LinkHash },
    #[error("strategy hash {found:?} is not the configured {expected:?}")]
    StrategyMismatch { found: LinkHash, expected: LinkHash },
    #[error("schema hash {found:?} is not the declared {expected:?} — link_fields drifted")]
    SchemaMismatch { found: LinkHash, expected: LinkHash },
    #[error("topic {topic:?} is engine-reserved and unknown to this version")]
    UnknownTopic { topic: TopicId },
    #[error("topic {topic:?} frames are {expected} bytes, datagram is {found}")]
    LengthMismatch {
        topic: TopicId,
        expected: usize,
        found: usize,
    },
    #[error("payload declares {count} values, capacity {capacity}")]
    FieldCountExceeded { count: u8, capacity: usize },
    #[error("book declares {count} levels, capacity {capacity}")]
    BookLevelsExceeded { count: u16, capacity: usize },
    #[error(
        "order snapshot declares detail_len={detail_len}, total_working={total_working}; capacities are detail={detail_capacity}, total={total_capacity}"
    )]
    OrderSnapshotCountsInvalid {
        detail_len: u8,
        total_working: u16,
        detail_capacity: usize,
        total_capacity: u16,
    },
    #[error("order snapshot detail contains terminal state {state:?}")]
    OrderSnapshotTerminalState { state: crate::hot::exec::OrderState },
    #[error("order snapshot detail repeats client id {client_id:?}")]
    OrderSnapshotDuplicate {
        client_id: crate::ids::ClientOrderId,
    },
    #[error("subscription names {count} topics, capacity {capacity}")]
    TopicCountExceeded { count: u8, capacity: usize },
    #[error("name is {found} bytes, capacity {LINK_NAME_LEN}")]
    NameTooLong { found: u8 },
    #[error("name bytes are not utf8")]
    NameNotUtf8,
    #[error("unknown {field} discriminant {value}")]
    UnknownDiscriminant { field: &'static str, value: u8 },
}

impl LinkDecodeError {
    pub(super) fn unknown(field: &'static str, value: u8) -> Self {
        Self::UnknownDiscriminant { field, value }
    }
}

pub(super) struct ByteWriter<'a> {
    bytes: &'a mut [u8],
    at: usize,
}

impl<'a> ByteWriter<'a> {
    pub(super) fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub(super) fn written(&self) -> usize {
        self.at
    }

    pub(super) fn write_u8(&mut self, value: u8) {
        self.bytes[self.at] = value;
        self.at += 1;
    }

    pub(super) fn write_bytes(&mut self, value: &[u8]) {
        self.bytes[self.at..self.at + value.len()].copy_from_slice(value);
        self.at += value.len();
    }

    pub(super) fn write_u16(&mut self, value: u16) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(super) fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(super) fn write_i32(&mut self, value: i32) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(super) fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(super) fn write_i64(&mut self, value: i64) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(super) fn write_f64(&mut self, value: f64) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(super) fn write_ts(&mut self, value: TsUs) {
        self.write_i64(value.micros());
    }

    pub(super) fn write_duration(&mut self, value: DurationUs) {
        self.write_i64(value.micros());
    }

    pub(super) fn write_hash(&mut self, value: LinkHash) {
        self.write_u64(value.0);
    }

    pub(super) fn write_name(&mut self, value: WireName) {
        self.write_u8(value.len);
        self.write_bytes(&value.bytes);
    }

    pub(super) fn write_optional_mantissa(&mut self, value: Option<i64>) {
        self.write_u8(if value.is_some() { PRESENT } else { ABSENT });
        self.write_i64(value.unwrap_or(0));
    }

    pub(super) fn write_optional_f64(&mut self, value: Option<f64>) {
        self.write_u8(if value.is_some() { PRESENT } else { ABSENT });
        self.write_f64(value.unwrap_or(0.0));
    }

    pub(super) fn write_optional_level(&mut self, value: Option<(Price, Qty)>) {
        self.write_optional_mantissa(value.map(|(price, _)| price.0));
        self.write_i64(value.map_or(0, |(_, qty)| qty.0));
    }

    /// Reused buffer must not leak previous datagram; future MAC covers these bytes.
    pub(super) fn pad_to(&mut self, end: usize) {
        self.bytes[self.at..end].fill(0);
        self.at = end;
    }
}

pub(super) struct ByteReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> ByteReader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// OOB read here = invariant violation (not bad input); frame length checked before body read.
    pub(super) fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0; N];
        out.copy_from_slice(&self.bytes[self.at..self.at + N]);
        self.at += N;
        out
    }

    pub(super) fn read_u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }

    pub(super) fn read_u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }

    pub(super) fn read_u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }

    pub(super) fn read_i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }

    pub(super) fn read_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }

    pub(super) fn read_i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take())
    }

    pub(super) fn read_f64(&mut self) -> f64 {
        f64::from_le_bytes(self.take())
    }

    pub(super) fn read_ts(&mut self) -> TsUs {
        TsUs::from_micros(self.read_i64())
    }

    pub(super) fn read_duration(&mut self) -> DurationUs {
        DurationUs::from_micros(self.read_i64())
    }

    pub(super) fn read_hash(&mut self) -> LinkHash {
        LinkHash(self.read_u64())
    }

    pub(super) fn read_name(&mut self) -> Result<WireName, LinkDecodeError> {
        let len = self.read_u8();
        let mut bytes: [u8; LINK_NAME_LEN] = self.take();
        if len as usize > LINK_NAME_LEN {
            return Err(LinkDecodeError::NameTooLong { found: len });
        }
        // Zero past len: garbage tail -> identical display, unequal compare, catalog upsert key mismatch
        bytes[len as usize..].fill(0);
        str::from_utf8(&bytes[..len as usize]).map_err(|_| LinkDecodeError::NameNotUtf8)?;
        Ok(WireName { bytes, len })
    }

    pub(super) fn read_optional_mantissa(&mut self) -> Result<Option<i64>, LinkDecodeError> {
        let tag = self.read_u8();
        let mantissa = self.read_i64();
        match tag {
            ABSENT => Ok(None),
            PRESENT => Ok(Some(mantissa)),
            _ => Err(LinkDecodeError::unknown("optional mantissa", tag)),
        }
    }

    pub(super) fn read_optional_f64(&mut self) -> Result<Option<f64>, LinkDecodeError> {
        let tag = self.read_u8();
        let value = self.read_f64();
        match tag {
            ABSENT => Ok(None),
            PRESENT => Ok(Some(value)),
            _ => Err(LinkDecodeError::unknown("optional f64", tag)),
        }
    }

    pub(super) fn read_optional_level(&mut self) -> Result<Option<(Price, Qty)>, LinkDecodeError> {
        let price = self.read_optional_mantissa()?;
        let qty = self.read_i64();
        Ok(price.map(|price| (Price(price), Qty(qty))))
    }
}
