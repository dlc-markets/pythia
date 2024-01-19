use std::num::ParseIntError;

use displaydoc::Display;
use thiserror::Error;

use std::io;

#[derive(Debug, Display, Error)]
pub enum PythiaConfigError {
    /// Secret key was not found
    NoSecretKey,

    /// Port is not a valid number: {0}
    PortInvalid(#[from] ParseIntError),

    /// Secret key must be a 32 bytes hex string
    InvalidSecretKey,

    /// Config file not found: {0}
    NoConfigFile(#[from] io::Error),

    /// Fail to parse config file: {0}
    ConfigParsing(#[from] serde_json::Error),
}
