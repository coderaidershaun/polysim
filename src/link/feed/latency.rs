//! Self-timing half of the UI event lane: three stage rows plus the input-ring backlog.

use crate::msg::ui::{UiLatencyCell, UiLatencyRow, UiLatencySummary};

use super::super::envelope::{ByteReader, ByteWriter, LinkDecodeError, OPTIONAL_F64_LEN};
use super::super::wire::{WireField, wire_struct};

const CELL_LEN: usize = 4 + 8;
const ROW_LEN: usize = 6 * CELL_LEN;
pub(super) const LATENCY_LEN: usize = 3 * ROW_LEN + OPTIONAL_F64_LEN;

/// Counts ride as u32. Saturating rather than widening the tail: the ten-minute window would need
/// seven million messages a second to reach the ceiling, and a displayed mean cannot tell by then.
fn write_count(writer: &mut ByteWriter<'_>, count: u64) {
    writer.write_u32(u32::try_from(count).unwrap_or(u32::MAX));
}

impl WireField for UiLatencyCell {
    fn write(&self, writer: &mut ByteWriter<'_>) {
        write_count(writer, self.count);
        self.sum_us.write(writer);
    }

    fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
        Ok(Self {
            count: u64::from(reader.read_u32()),
            sum_us: reader.read_i64(),
        })
    }
}

wire_struct! {
    UiLatencyRow {
        exchange_to_received,
        received_to_queued,
        queue_wait,
        processing,
        end_to_end,
        order_round_trip,
    }
}

impl WireField for UiLatencySummary {
    fn write(&self, writer: &mut ByteWriter<'_>) {
        self.market_data.write(writer);
        self.execution.write(writer);
        self.hot_path.write(writer);
        writer.write_optional_f64(self.backlog_ema);
    }

    fn read(reader: &mut ByteReader<'_>) -> Result<Self, LinkDecodeError> {
        Ok(Self {
            market_data: WireField::read(reader)?,
            execution: WireField::read(reader)?,
            hot_path: WireField::read(reader)?,
            backlog_ema: reader.read_optional_f64()?,
        })
    }
}
