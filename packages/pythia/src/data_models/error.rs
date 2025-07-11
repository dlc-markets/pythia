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
}
