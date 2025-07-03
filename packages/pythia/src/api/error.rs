use displaydoc::Display;
use std::io;
use thiserror::Error;

use crate::{data_models::asset_pair::AssetPair, oracle::error::OracleError};

#[derive(Debug, Display, Error)]
pub enum PythiaApiError {
    /// asset pair {0} not recorded
    UnrecordedAssetPair(AssetPair),

    /// datetime RFC3339 parsing with {0}
    DatetimeParsing(#[from] chrono::format::ParseError),

    /// database error: {0}
    DatabaseFail(#[from] sqlx::Error),

    /// oracle Error: {0}
    OracleFail(#[from] OracleError),

    /// event {0} does not exist.
    OracleEventNotFoundError(String),

    /// oracle is empty.
    OracleEmpty,

    /// could not start socket: {0}
    SocketUnavailable(#[from] io::Error),

    /// JSON-RPC parsing error: {0}
    JsonRpcParsingError(#[from] serde_json::Error),

    /// WebSocket communication error: {0}
    #[cfg(test)]
    WebSocketError(String),

    #[cfg(not(test))]
    /// WebSocket closed
    WebSocketError(#[from] actix_ws::Closed),

    /// An expiry related event can only have a timestamps less or equal to the expiry.
    TimestampGreaterThanExpiry,
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
            PythiaApiError::OracleEmpty => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            PythiaApiError::JsonRpcParsingError(_) => actix_web::http::StatusCode::BAD_REQUEST,
            PythiaApiError::WebSocketError(_) => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            PythiaApiError::TimestampGreaterThanExpiry => actix_web::http::StatusCode::BAD_REQUEST,
        }
    }
}

#[cfg(test)]
impl From<actix_ws::ProtocolError> for PythiaApiError {
    fn from(err: actix_ws::ProtocolError) -> Self {
        PythiaApiError::WebSocketError(err.to_string())
    }
}

#[cfg(test)]
impl From<actix_ws::Closed> for PythiaApiError {
    fn from(err: actix_ws::Closed) -> Self {
        PythiaApiError::WebSocketError(format!("WebSocket closed: {err}"))
    }
}
