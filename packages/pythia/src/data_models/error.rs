use displaydoc::Display;
use std::str::Utf8Error;
use thiserror::Error;

use crate::data_models::legs_combo::error::LegsComboError;

#[derive(Debug, Display, Error, PartialEq)]
pub enum ParsingError {
    /// invalid length: expected {expected}, actual {actual}
    InvalidLength { expected: usize, actual: usize },

    /// {0}
    InvalidUtf8(#[from] Utf8Error),

    /// Error parsing the provided expiry: {0}
    InvalidDate(#[from] chrono::format::ParseError),

    /// Expiry must be uppercased %d%b%y chrono format
    InvalidExpiryCase,

    /// EventId must be one of a forward or delivery event id
    NoExpiryInEventId,

    /// {0}
    InvalidLegCombo(#[from] LegsComboError),
}
