use std::num::ParseIntError;

use crate::{oracle::OracleError, AssetPair};
use displaydoc::Display;
use thiserror::Error;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Display, Error)]
pub enum PythiaError {
    /// asset pair {0} not recorded
    UnrecordedAssetPairError(AssetPair),

    /// datetime RFC3339 parsing error: {0}
    DatetimeParseError(#[from] time::error::Parse),

    /// oracle event with maturation {0} not found
    OracleEventNotFoundError(String),

    /// database error: {0}
    DatabaseError(#[from] sqlx::Error),

    /// Oracle Error: {0}
    OracleError(#[from] OracleError),

    /// Port is not a valid number: {0}
    ConfigError(#[from] ParseIntError),

    /// Secret key was not found
    NoSecretKey,

    /// Secret key must be a 32 bytes hex string
    InvalidSecretKey,
}

impl actix_web::error::ResponseError for PythiaError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        if let PythiaError::DatetimeParseError(_) = self {
            return actix_web::http::StatusCode::BAD_REQUEST;
        }
        actix_web::http::StatusCode::INTERNAL_SERVER_ERROR
    }
}
