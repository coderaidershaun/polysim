//! The actor's kline driver: REST-backfill at connect, forward live WS updates, and fill a hole by
//! REST when [`KlineOutcome::Gap`] reports one. The dedupe/gap rules live in the `kline` module.

use std::time::Instant;

use crate::adapters::binance::kline::{KlineOutcome, KlineSequencer};
use crate::adapters::binance::parse::{
    ParseContext, RestKlineTail, parse_kline, parse_rest_klines,
};
use crate::adapters::binance::rest::{KlineQuery, RestRequest};
use crate::config::KlineInterval;
use crate::ids::InstrumentId;
use crate::msg::inbound::KlineEvent;
use crate::time::TsUs;

use super::Actor;

const GAP_KLINE_LIMIT: u32 = 1000;

impl Actor {
    pub(super) async fn handle_kline(&mut self, data: &str, ctx: ParseContext) {
        let event = match parse_kline(data, ctx) {
            Ok(event) => event,
            Err(error) => return self.on_parse_error(error),
        };
        let key = (event.instrument, event.interval);

        let mut out = Vec::new();
        let outcome = {
            let Some(sequencer) = self.kline_states.get_mut(&key) else {
                return self.on_unroutable_frame(format_args!(
                    "{} klines on instrument {}",
                    event.interval.as_str(),
                    event.instrument.0
                ));
            };
            sequencer.on_live(event, &mut |message| out.push(message))
        };
        self.flush(out);

        if let KlineOutcome::Gap {
            missing_from_open_ts_us,
            next_open_ts_us,
        } = outcome
        {
            self.fill_kline_gap(key, missing_from_open_ts_us, next_open_ts_us, event)
                .await;
        }
    }

    async fn fill_kline_gap(
        &mut self,
        key: (InstrumentId, KlineInterval),
        missing_from_open_ts_us: TsUs,
        next_open_ts_us: TsUs,
        live: KlineEvent,
    ) {
        if self.rest_quiet.is_active(Instant::now()) {
            return;
        }
        let (instrument, interval) = key;
        let Some(symbol) = self.venue_symbol(instrument) else {
            return self.on_unknown_symbol(instrument, "its kline gap fill");
        };
        let request = RestRequest::Klines(KlineQuery {
            symbol,
            interval,
            limit: GAP_KLINE_LIMIT,
            start_ts_ms: Some(missing_from_open_ts_us.micros() / 1_000),
            end_ts_ms: Some(next_open_ts_us.micros() / 1_000 - 1),
        });
        let json = match self.fetch_text_beating(&request).await {
            Ok(json) => json,
            Err(error) => {
                self.on_rest_error(&error, &format!("kline gap {}", instrument.0));
                return;
            }
        };
        let ctx = ParseContext {
            instrument,
            received_ts_us: self.clock.now(),
        };
        let events = match parse_rest_klines(&json, ctx, interval, RestKlineTail::AllClosed) {
            Ok(events) => events,
            Err(error) => return self.on_parse_error(error),
        };

        let mut out = Vec::new();
        if let Some(sequencer) = self.kline_states.get_mut(&key) {
            sequencer.on_backfill(&events, &mut |message| out.push(message));
            sequencer.on_live(live, &mut |message| out.push(message));
        }
        self.flush(out);
    }

    pub(super) async fn backfill_klines(&mut self) {
        for target in self.kline_targets.clone() {
            if self.rest_quiet.is_active(Instant::now()) {
                continue;
            }
            let key = (target.instrument, target.interval);
            // Window must span buffered deltas or sequencer misses closed candles. Reconnect uses anchor + backfill_limit; first connect fetches newest.
            let start_ts_ms = self
                .kline_states
                .get(&key)
                .and_then(KlineSequencer::next_expected_open_ts_us)
                .map(|open_ts_us| open_ts_us.micros() / 1_000);
            let Some(symbol) = self.venue_symbol(target.instrument) else {
                self.on_unknown_symbol(target.instrument, "its kline backfill");
                continue;
            };
            let request = RestRequest::Klines(KlineQuery {
                symbol,
                interval: target.interval,
                limit: target.backfill_limit,
                start_ts_ms,
                end_ts_ms: None,
            });
            let json = match self.fetch_text_beating(&request).await {
                Ok(json) => json,
                Err(error) => {
                    self.on_rest_error(&error, &format!("kline backfill {}", target.instrument.0));
                    continue;
                }
            };
            let ctx = ParseContext {
                instrument: target.instrument,
                received_ts_us: self.clock.now(),
            };
            let events = match parse_rest_klines(
                &json,
                ctx,
                target.interval,
                RestKlineTail::OpenCandleForming,
            ) {
                Ok(events) => events,
                Err(error) => {
                    self.on_parse_error(error);
                    continue;
                }
            };
            let mut out = Vec::new();
            if let Some(sequencer) = self.kline_states.get_mut(&key) {
                sequencer.on_backfill(&events, &mut |message| out.push(message));
            }
            self.flush(out);
        }
    }
}
