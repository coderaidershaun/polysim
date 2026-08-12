//! Configuration for the simulated venue.

use serde::Deserialize;

use super::ConfigError;
use super::execution::{check_bounded_milliseconds, check_non_zero, check_nonnegative_amount};

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimConfig {
    #[serde(default = "default_order_entry_latency_ms")]
    pub order_entry_latency_ms: u64,
    #[serde(default = "default_ack_latency_ms")]
    pub ack_latency_ms: u64,
    #[serde(default = "default_max_market_data_delay_ms")]
    pub max_market_data_delay_ms: u64,
    #[serde(default = "default_sim_heartbeat_ms")]
    pub heartbeat_ms: u64,
    #[serde(default = "default_opening_base_balance")]
    pub opening_base_balance: f64,
    #[serde(default = "default_opening_quote_balance")]
    pub opening_quote_balance: f64,
    #[serde(default = "default_maker_fee_bps")]
    pub maker_fee_bps: f64,
}

impl SimConfig {
    pub(super) fn validate(
        &self,
        spin_interval_us: u64,
        inflight_timeout_ms: u64,
    ) -> Result<(), ConfigError> {
        check_nonnegative_amount(
            "execution.sim.opening_base_balance",
            self.opening_base_balance,
        )?;
        check_nonnegative_amount(
            "execution.sim.opening_quote_balance",
            self.opening_quote_balance,
        )?;
        check_nonnegative_amount("execution.sim.maker_fee_bps", self.maker_fee_bps)?;
        if self.maker_fee_bps > 10_000.0 || self.maker_fee_bps.fract() != 0.0 {
            return Err(ConfigError::Invalid {
                field: "execution.sim.maker_fee_bps",
                value: self.maker_fee_bps.to_string().into(),
                expected: "a whole number of basis points in 0..=10000",
            });
        }

        check_non_zero(
            "execution.sim.max_market_data_delay_ms",
            self.max_market_data_delay_ms,
        )?;
        check_non_zero("execution.sim.heartbeat_ms", self.heartbeat_ms)?;
        for (field, value) in [
            (
                "execution.sim.order_entry_latency_ms",
                self.order_entry_latency_ms,
            ),
            ("execution.sim.ack_latency_ms", self.ack_latency_ms),
            (
                "execution.sim.max_market_data_delay_ms",
                self.max_market_data_delay_ms,
            ),
            ("execution.sim.heartbeat_ms", self.heartbeat_ms),
        ] {
            check_bounded_milliseconds(field, value)?;
        }

        let heartbeat_us = u128::from(self.heartbeat_ms) * 1_000;
        let slowest_heartbeat_us = heartbeat_us.max(u128::from(spin_interval_us));
        let worst_us = [
            u128::from(self.order_entry_latency_ms) * 1_000,
            u128::from(self.ack_latency_ms) * 1_000,
            u128::from(self.max_market_data_delay_ms) * 1_000,
            slowest_heartbeat_us,
            slowest_heartbeat_us,
        ]
        .into_iter()
        .sum::<u128>();
        let timeout_us = u128::from(inflight_timeout_ms) * 1_000;
        if worst_us >= timeout_us {
            return Err(ConfigError::Invalid {
                field: "execution.sim",
                value: worst_us.div_ceil(1_000).to_string().into(),
                expected: "entry + ack + market-data delay + two slowest producer heartbeats \
                           STRICTLY below execution.inflight_timeout_ms",
            });
        }
        Ok(())
    }
}

fn default_order_entry_latency_ms() -> u64 {
    15
}

fn default_ack_latency_ms() -> u64 {
    15
}

fn default_max_market_data_delay_ms() -> u64 {
    1_000
}

fn default_sim_heartbeat_ms() -> u64 {
    100
}

fn default_opening_base_balance() -> f64 {
    0.01
}

fn default_opening_quote_balance() -> f64 {
    1_000.0
}

fn default_maker_fee_bps() -> f64 {
    10.0
}
