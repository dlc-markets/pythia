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

    /// Invalid char input: got {actual} expected {expected}
    InvalidChar {
        expected: &'static str,
        actual: String,
    },
    /// Expiry must be uppercased %d%b%y chrono format
    InvalidExpiryCase,

    /// EventId must be one of a forward or delivery event id
    NoExpiryInEventId,
}
