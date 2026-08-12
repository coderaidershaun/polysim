//! One declaration per wire record: writer and reader are generated from the same field list, so no
//! edit can teach one half a layout the other never learned.
//! Macro bodies sit outside the fmt gate (rustfmt skips brace-delimited item macros) — hand-formatted.

use crate::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty};
use crate::msg::persist::FeatureId;
use crate::time::{DurationUs, TsUs};

use super::envelope::{ByteReader, ByteWriter, LinkDecodeError, LinkHash, TopicId, WireName};

/// Reading is fallible for every type so one declaration form serves plain fields and tagged ones
/// alike; the plain cases return `Ok` unconditionally.
pub(super) trait WireField: Sized {
    fn write(&self, writer: &mut ByteWriter<'_>);
    fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError>;
}

macro_rules! wire_primitive {
    ($($ty:ty => $write:ident / $read:ident),+ $(,)?) => {
        $(impl WireField for $ty {
            #[inline]
            fn write(&self, writer: &mut ByteWriter<'_>) {
                writer.$write(*self);
            }

            #[inline]
            fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
                Ok(reader.$read())
            }
        })+
    };
}

macro_rules! wire_newtype {
    ($($ty:ident => $inner:ty),+ $(,)?) => {
        $(impl WireField for $ty {
            #[inline]
            fn write(&self, writer: &mut ByteWriter<'_>) {
                self.0.write(writer);
            }

            #[inline]
            fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
                Ok($ty(<$inner as WireField>::read(reader)?))
            }
        })+
    };
}

/// A missing mantissa still occupies its eight bytes, so the tail width never depends on the data.
macro_rules! wire_optional_mantissa {
    ($($ty:ident),+ $(,)?) => {
        $(impl WireField for Option<$ty> {
            fn write(&self, writer: &mut ByteWriter<'_>) {
                writer.write_optional_mantissa(self.map(|value| value.0));
            }

            fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
                Ok(reader.read_optional_mantissa()?.map($ty))
            }
        })+
    };
}

/// Each value is parenthesised so the macro can splice the same tokens into a match arm and into the
/// expression that rebuilds it. Matching on the value keeps the compiler's exhaustiveness check: a
/// variant added upstream fails the build here rather than reaching the wire untagged.
macro_rules! wire_enum {
    ($ty:ty, $label:literal; $( ($($value:tt)+) = $tag:literal ),+ $(,)?) => {
        impl WireField for $ty {
            fn write(&self, writer: &mut ByteWriter<'_>) {
                writer.write_u8(match *self {
                    $( $($value)+ => $tag, )+
                });
            }

            fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
                let tag = reader.read_u8();
                match tag {
                    $( $tag => Ok($($value)+), )+
                    _ => Err(LinkDecodeError::unknown($label, tag)),
                }
            }
        }
    };
}

/// Fields ride the wire in declaration order. A field may carry a trailing `after <expr>` that runs
/// the instant it lands, which is how a length is refused before the values it sizes are trusted.
macro_rules! wire_struct {
    ($ty:ident { $($field:ident $(after $checked:expr)?),+ $(,)? }) => {
        impl WireField for $ty {
            fn write(&self, writer: &mut ByteWriter<'_>) {
                $( self.$field.write(writer); )+
            }

            fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
                $(
                    let $field = WireField::read(reader)?;
                    $( $checked; )?
                )+
                Ok(Self { $($field),+ })
            }
        }
    };
}

/// Fixed-width run of one kind. The seed is named rather than derived: a `Default` on a wire type
/// would be a public promise made for a private convenience.
macro_rules! wire_array {
    ($($ty:ty; $len:expr; $empty:expr),+ $(,)?) => {
        $(impl WireField for [$ty; $len] {
            fn write(&self, writer: &mut ByteWriter<'_>) {
                for item in self {
                    item.write(writer);
                }
            }

            fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
                let mut items = [$empty; $len];
                for item in &mut items {
                    *item = WireField::read(reader)?;
                }
                Ok(items)
            }
        })+
    };
}

pub(super) use {wire_array, wire_enum, wire_struct};

wire_primitive! {
    u8 => write_u8 / read_u8,
    u16 => write_u16 / read_u16,
    u32 => write_u32 / read_u32,
    i32 => write_i32 / read_i32,
    u64 => write_u64 / read_u64,
    i64 => write_i64 / read_i64,
    f64 => write_f64 / read_f64,
    TsUs => write_ts / read_ts,
    DurationUs => write_duration / read_duration,
    LinkHash => write_hash / read_hash,
}

wire_newtype! {
    TopicId => u16,
    InstrumentId => u16,
    AssetId => u16,
    FeatureId => u16,
    ClientOrderId => u64,
    Price => i64,
    Qty => i64,
}

wire_optional_mantissa!(Price, Qty);

impl WireField for WireName {
    fn write(&self, writer: &mut ByteWriter<'_>) {
        writer.write_name(*self);
    }

    fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
        reader.read_name()
    }
}

impl WireField for Option<(Price, Qty)> {
    fn write(&self, writer: &mut ByteWriter<'_>) {
        writer.write_optional_level(*self);
    }

    fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
        reader.read_optional_level()
    }
}
