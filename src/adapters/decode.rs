//! Every venue's JSON boundary meets the same two hazards: a frame serde cannot read, and a decimal
//! that will not fit an `i64@1e-8` mantissa. One fault type so the fatal-vs-drop verdict cannot
//! drift between venues.

use serde::Deserialize;

use crate::ids::{DecimalError, Price, Qty};

/// `MantissaOverflow` is fatal — a venue number this scale cannot hold escalates, never truncates.
#[derive(thiserror::Error, Debug)]
pub enum DecimalFault {
    #[error("malformed {frame} frame: {source}")]
    Json {
        frame: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("malformed decimal in {field}: {value:?} ({reason})")]
    Decimal {
        field: &'static str,
        value: Box<str>,
        reason: &'static str,
    },
    #[error("mantissa overflow in {field}: {value:?} exceeds i64@1e-8 scale")]
    MantissaOverflow {
        field: &'static str,
        value: Box<str>,
    },
}

impl DecimalFault {
    pub fn is_fatal(&self) -> bool {
        matches!(self, DecimalFault::MantissaOverflow { .. })
    }
}

/// The venue's own word in "malformed {frame} frame" — a newtype so it cannot be handed to a
/// decoder where the payload belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JsonFrame(pub(crate) &'static str);

impl JsonFrame {
    pub(crate) fn decode<T: for<'de> Deserialize<'de>>(
        self,
        json: &str,
    ) -> Result<T, DecimalFault> {
        serde_json::from_str(json).map_err(|source| self.fault(source))
    }

    pub(crate) fn decode_value<T: for<'de> Deserialize<'de>>(
        self,
        value: serde_json::Value,
    ) -> Result<T, DecimalFault> {
        serde_json::from_value(value).map_err(|source| self.fault(source))
    }

    pub(crate) fn fault(self, source: serde_json::Error) -> DecimalFault {
        DecimalFault::Json {
            frame: self.0,
            source,
        }
    }
}

pub(crate) fn price_field(field: &'static str, text: &str) -> Result<Price, DecimalFault> {
    Price::parse_decimal(text).map_err(|error| decimal_fault(field, text, error))
}

pub(crate) fn qty_field(field: &'static str, text: &str) -> Result<Qty, DecimalFault> {
    Qty::parse_decimal(text).map_err(|error| decimal_fault(field, text, error))
}

pub(crate) fn mantissa_field(field: &'static str, text: &str) -> Result<i64, DecimalFault> {
    Qty::parse_decimal(text)
        .map(|qty| qty.0)
        .map_err(|error| decimal_fault(field, text, error))
}

fn decimal_fault(field: &'static str, text: &str, error: DecimalError) -> DecimalFault {
    match error {
        DecimalError::Overflow { .. } => DecimalFault::MantissaOverflow {
            field,
            value: text.into(),
        },
        DecimalError::Empty => DecimalFault::Decimal {
            field,
            value: text.into(),
            reason: "empty decimal string",
        },
        DecimalError::InvalidChar { .. } => DecimalFault::Decimal {
            field,
            value: text.into(),
            reason: "non-numeric character",
        },
        DecimalError::TooPrecise { .. } => DecimalFault::Decimal {
            field,
            value: text.into(),
            reason: "more than 8 decimal places",
        },
    }
}
