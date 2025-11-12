use displaydoc::Display;
use std::str::Utf8Error;
use thiserror::Error;

#[derive(Debug, Display, Error)]
pub enum ParsingError {
    /// invalid length: expected {expected}, actual {actual}
    InvalidLength { expected: usize, actual: usize },

    /// {0}
    InvalidUtf8(#[from] Utf8Error),

    /// Error parsing the provided expiry: {0}
    InvalidDate(#[from] chrono::format::ParseError),

    /// Invalid timestamp: {0}
    InvalidTimestamp(#[from] std::num::ParseIntError),

    /// Error parsing the provided asset pair: got {actual} expected {expected}
    InvalidAssetPair {
        expected: &'static str,
        actual: String,
    },
}
