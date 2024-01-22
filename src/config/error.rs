use std::num::ParseIntError;

use displaydoc::Display;
use thiserror::Error;

use std::io;

#[derive(Debug, Display, Error)]
pub(crate) enum PythiaConfigError {
    /// secret key was not found
    NoSecretKey,

    /// port {0} is not a valid number
    PortInvalid(#[from] ParseIntError),

    /// secret key must be a 32 bytes hex string
    InvalidSecretKey,

    /// config file not found with path {0}
    NoConfigFile(#[from] io::Error),

    /// fail to parse config file with path {0}
    ConfigParsing(#[from] serde_json::Error),
}
