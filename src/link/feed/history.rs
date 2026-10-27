//! The history topic's body: a chunk of the engine's remembered chart inputs. Every sample pads to
//! one width, so the sample array has a fixed layout and the datagram a fixed length.

use crate::ids::Side;
use crate::msg::ui::{TopOfBook, UI_HISTORY_SAMPLES, UiHistory, UiHistorySample};

use super::super::envelope::{
    ByteReader, ByteWriter, ENVELOPE_LEN, LINK_MAX_DATAGRAM, LinkDecodeError,
};
use super::super::wire::{WireField, wire_array, wire_struct};

const TAG_LEN: usize = 1;
const STAMP_LEN: usize = 8;
const VALUE_LEN: usize = 8;
const FLAG_LEN: usize = 1;
const SIDE_LEN: usize = 1;

const MID_LEN: usize = TAG_LEN + STAMP_LEN + 2 * VALUE_LEN + FLAG_LEN;
const POSITION_LEN: usize = TAG_LEN + STAMP_LEN + 2 * VALUE_LEN;
const FILL_LEN: usize = TAG_LEN + STAMP_LEN + SIDE_LEN + VALUE_LEN;
const SAMPLE_LEN: usize = MID_LEN;

const HEAD_LEN: usize = 2 + 8 + 8 + 1;
pub(in crate::link) const HISTORY_BODY_LEN: usize = HEAD_LEN + UI_HISTORY_SAMPLES * SAMPLE_LEN;

const _: () = assert!(ENVELOPE_LEN + HISTORY_BODY_LEN <= LINK_MAX_DATAGRAM);
const _: () = assert!(POSITION_LEN <= SAMPLE_LEN);
const _: () = assert!(FILL_LEN <= SAMPLE_LEN);

const MID: u8 = 1;
const POSITION: u8 = 2;
const FILL: u8 = 3;

impl WireField for UiHistorySample {
    fn write(&self, writer: &mut ByteWriter<'_>) {
        let end = writer.written() + SAMPLE_LEN;
        match *self {
            Self::Mid {
                event_ts_us,
                top,
                has_gap_before,
            } => {
                MID.write(writer);
                event_ts_us.write(writer);
                top.bid.write(writer);
                top.ask.write(writer);
                u8::from(has_gap_before).write(writer);
            }
            Self::Position {
                event_ts_us,
                exposure_quote,
                pnl_quote,
            } => {
                POSITION.write(writer);
                event_ts_us.write(writer);
                exposure_quote.write(writer);
                pnl_quote.write(writer);
            }
            Self::Fill {
                event_ts_us,
                side,
                price,
            } => {
                FILL.write(writer);
                event_ts_us.write(writer);
                side.write(writer);
                price.write(writer);
            }
        }
        writer.pad_to(end);
    }

    fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
        let end = reader.consumed() + SAMPLE_LEN;
        let tag = u8::read(reader)?;
        let event_ts_us = WireField::read(reader)?;
        let sample = match tag {
            MID => Self::Mid {
                event_ts_us,
                top: TopOfBook {
                    bid: WireField::read(reader)?,
                    ask: WireField::read(reader)?,
                },
                has_gap_before: read_gap_flag(reader)?,
            },
            POSITION => Self::Position {
                event_ts_us,
                exposure_quote: WireField::read(reader)?,
                pnl_quote: WireField::read(reader)?,
            },
            FILL => Self::Fill {
                event_ts_us,
                side: Side::read(reader)?,
                price: WireField::read(reader)?,
            },
            _ => return Err(LinkDecodeError::unknown("history sample kind", tag)),
        };
        reader.skip_to(end);
        Ok(sample)
    }
}

fn read_gap_flag(reader: &mut ByteReader<'_>) -> Result<bool, LinkDecodeError> {
    match u8::read(reader)? {
        0 => Ok(false),
        1 => Ok(true),
        flag => Err(LinkDecodeError::unknown("gap flag", flag)),
    }
}

wire_array! {
    UiHistorySample; UI_HISTORY_SAMPLES; UiHistorySample::EMPTY,
}

wire_struct! {
    UiHistory {
        instrument,
        cutoff_ts_us,
        first_index,
        len after validate_history_len(len)?,
        samples,
    }
}

/// A count past capacity would let a remote peer make a reader index out of bounds, so it is
/// refused once here.
fn validate_history_len(count: u8) -> Result<(), LinkDecodeError> {
    if usize::from(count) <= UI_HISTORY_SAMPLES {
        return Ok(());
    }
    Err(LinkDecodeError::HistoryLenExceeded {
        count,
        capacity: UI_HISTORY_SAMPLES,
    })
}
