use displaydoc::Display;
use thiserror::Error;

use crate::{api::error::PythiaApiError, config::error::PythiaConfigError, oracle::OracleError};

#[derive(Debug, Display, Error)]
pub enum PythiaError {
    /// Error from Api: {0}
    Api(#[from] PythiaApiError),

    /// Configuration Error: {0}
    Config(#[from] PythiaConfigError),

    /// Oracle failed: {0}
    Oracle(#[from] OracleError),

    /// Postgres Error: {0}
    Postgres(#[from] sqlx::Error),
}
