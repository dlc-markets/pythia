use displaydoc::Display;

use thiserror::Error;

use crate::{config::AssetPair, oracle::OracleError};

use std::io;

#[derive(Debug, Display, Error)]
pub enum PythiaApiError {
    /// asset pair {0} not recorded
    UnrecordedAssetPair(AssetPair),

    /// datetime RFC3339 parsing error: {0}
    DatetimeParsing(#[from] chrono::format::ParseError),

    /// database error: {0}
    DatabaseFail(#[from] sqlx::Error),

    /// Oracle Error: {0}
    OracleFail(#[from] OracleError),

    /// This event does not exist: {0}
    OracleEventNotFoundError(String),

    /// Could not start socket: {0}
    SocketUnavailable(#[from] io::Error),
}

impl actix_web::error::ResponseError for PythiaApiError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            PythiaApiError::UnrecordedAssetPair(_) => actix_web::http::StatusCode::NOT_FOUND,
            PythiaApiError::DatetimeParsing(_) => actix_web::http::StatusCode::BAD_REQUEST,
            PythiaApiError::DatabaseFail(_) => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            PythiaApiError::OracleFail(_) => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            PythiaApiError::OracleEventNotFoundError(_) => actix_web::http::StatusCode::NOT_FOUND,
            PythiaApiError::SocketUnavailable(_) => {
                actix_web::http::StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}
