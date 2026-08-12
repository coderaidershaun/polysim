//! The workstation's view of the engine it is attached to: whether the heartbeat is still arriving,
//! what run state the engine reports against the one this window asserts, and the run catalog
//! rebuilt from the loose per-item frames the link announces. No socket and no clock of its own, so
//! the transport thread and the link bar read one source of truth rather than two.

use std::net::SocketAddr;
use std::time::Duration;

use crate::link::{
    CatalogFeature, CatalogInstrument, LINK_SUBSCRIPTION_TTL, Lifecycle, RunPhase, RunState,
};
use crate::msg::ui::{UiCatalog, UiInstrument};
use crate::shutdown::RunAssertion;
use crate::time::TsUs;

/// Silence past which a peer's feed reads as stale rather than merely quiet. The engine announces
/// its lifecycle once a second, and this matches the far side's own subscription TTL — so "I have
/// stopped hearing the engine" and "the engine has forgotten me" surface at the same moment.
pub(crate) const PEER_SILENCE_LIMIT: Duration =
    Duration::from_micros(LINK_SUBSCRIPTION_TTL.micros() as u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionState {
    /// Subscribed, nothing heard back. A trading engine that is not running looks exactly like this.
    Connecting,
    Live,
    /// Heard once, then nothing for [`PEER_SILENCE_LIMIT`]. UDP silence is otherwise
    /// indistinguishable from an engine with nothing to say, which is the whole reason this state
    /// is on screen.
    Stale,
}

impl ConnectionState {
    pub fn from_silence(since_last_frame: Option<Duration>) -> Self {
        match since_last_frame {
            None => Self::Connecting,
            Some(silence) if silence >= PEER_SILENCE_LIMIT => Self::Stale,
            Some(_) => Self::Live,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlVerdict {
    NoOpinion,
    Pending,
    Applied,
    Lost { holder_epoch: u64 },
}

#[derive(Debug)]
pub struct Controller {
    boot_epoch: u64,
    assertions: u64,
    asserted_state: Option<RunState>,
}

impl Controller {
    pub fn new(boot_ts_us: TsUs) -> Self {
        Self {
            boot_epoch: boot_ts_us.micros().max(0) as u64,
            assertions: 0,
            asserted_state: None,
        }
    }

    pub fn assert(&mut self, state: RunState) {
        self.assertions += 1;
        self.asserted_state = Some(state);
    }

    pub fn release(&mut self) {
        self.asserted_state = None;
    }

    pub fn assertion(&self) -> RunAssertion {
        match self.asserted_state {
            None => RunAssertion::INITIAL,
            Some(state) => RunAssertion {
                state,
                epoch: self.boot_epoch.saturating_add(self.assertions),
            },
        }
    }

    pub fn asserted(&self) -> Option<RunState> {
        self.asserted_state
    }

    pub fn verdict(&self, reported: Option<Lifecycle>) -> ControlVerdict {
        if self.asserted_state.is_none() {
            return ControlVerdict::NoOpinion;
        }
        let Some(reported) = reported else {
            return ControlVerdict::Pending;
        };
        let mine = self.assertion().epoch;
        if reported.acknowledged_epoch > mine {
            return ControlVerdict::Lost {
                holder_epoch: reported.acknowledged_epoch,
            };
        }
        let is_applied =
            reported.acknowledged_epoch == mine && self.asserted_state == Some(reported.run_state);
        if is_applied { ControlVerdict::Applied } else { ControlVerdict::Pending }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LinkStatus {
    pub peer: SocketAddr,
    pub peer_index: usize,
    pub peer_count: usize,
    pub session: u64,
    pub connection: ConnectionState,
    pub phase: Option<RunPhase>,
    pub reported_state: Option<RunState>,
    pub asserted_state: Option<RunState>,
    pub control: ControlVerdict,
}

impl LinkStatus {
    pub fn next_assertion(&self) -> RunState {
        match self.asserted_state.or(self.reported_state) {
            Some(RunState::Idle) => RunState::Running,
            Some(RunState::Running) | None => RunState::Idle,
        }
    }
}

#[derive(Debug, Default)]
pub struct CatalogAssembly {
    catalog_ts_us: Option<TsUs>,
    instruments: Vec<CatalogInstrument>,
    features: Vec<CatalogFeature>,
    instrument_total: Option<u16>,
}

impl CatalogAssembly {
    pub fn new() -> Self {
        Self::default()
    }

    /// When the engine stamped the catalog these frames belong to. Not the control epoch: that is
    /// an assertion ordinal, and comparing the two numbers would mean nothing.
    pub fn catalog_ts_us(&self) -> Option<TsUs> {
        self.catalog_ts_us
    }

    pub fn accept_instrument(&mut self, frame: CatalogInstrument) {
        if !self.adopt(frame.catalog_ts_us) {
            return;
        }
        self.instrument_total = Some(frame.total_count);
        let existing_index = self
            .instruments
            .iter()
            .position(|held| held.instrument == frame.instrument);
        match existing_index {
            Some(index) => self.instruments[index] = frame,
            None if self.instruments.len() < usize::from(frame.total_count) => {
                self.instruments.push(frame);
            }
            None => {}
        }
    }

    pub fn accept_feature(&mut self, frame: CatalogFeature) {
        if frame.feature.0 >= frame.total_count || !self.adopt(frame.catalog_ts_us) {
            return;
        }
        match self
            .features
            .iter()
            .position(|held| held.feature == frame.feature)
        {
            Some(index) => self.features[index] = frame,
            None => self.features.push(frame),
        }
    }

    pub fn build(
        &self,
        strategy_id: &str,
        peer: SocketAddr,
        reported: Lifecycle,
    ) -> Option<UiCatalog> {
        if !self.is_complete(reported.feature_count) {
            return None;
        }
        let mut rows: Vec<&CatalogInstrument> = self.instruments.iter().collect();
        rows.sort_unstable_by_key(|row| row.instrument.0);
        let instruments = rows
            .into_iter()
            .map(|row| UiInstrument {
                instrument_id: row.instrument,
                display: row.display.as_str().into(),
                base: row.base.as_str().into(),
                quote: row.quote.as_str().into(),
                base_asset: row.base_asset,
                quote_asset: row.quote_asset,
                tick_size: row.tick_size,
                lot_size: row.lot_size,
                qty_scale: row.qty_scale,
            })
            .collect();

        let mut feature_names: Vec<Box<str>> = vec![Box::default(); self.features.len()];
        for frame in &self.features {
            let slot = feature_names.get_mut(usize::from(frame.feature.0))?;
            *slot = frame.name.as_str().into();
        }

        Some(UiCatalog {
            strategy_id: strategy_id.into(),
            window_title: format!("Polysim - {strategy_id} @ {peer}").into(),
            // From the heartbeat, never inferred: an engine this window cannot hear has no mode.
            execution_mode: reported.execution_mode,
            // A peer reporting a negative cadence is nonsense the model reads as "no cadence",
            // rather than wrapping it into an enormous one.
            spin_interval_us: u64::try_from(reported.spin_interval_us.micros()).unwrap_or(0),
            instruments,
            feature_names,
        })
    }

    fn is_complete(&self, feature_count: u16) -> bool {
        self.instrument_total
            .is_some_and(|total| self.instruments.len() == usize::from(total))
            && self.features.len() == usize::from(feature_count)
    }

    fn adopt(&mut self, catalog_ts_us: TsUs) -> bool {
        match self.catalog_ts_us {
            Some(held) if catalog_ts_us < held => false,
            Some(held) if catalog_ts_us == held => true,
            _ => {
                self.catalog_ts_us = Some(catalog_ts_us);
                self.instruments.clear();
                self.features.clear();
                self.instrument_total = None;
                true
            }
        }
    }
}
