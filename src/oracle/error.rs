use displaydoc::Display;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, OracleError>;

#[derive(Debug, Display, Error)]
pub enum OracleError {
    /// database error: {0}
    DatabaseError(#[from] sqlx::Error),

    /// Event already attested
    AlreadyAttestatedError(String),

    /// pricefeed error: {0}
    PriceFeedError(#[from] crate::pricefeeds::PriceFeedError),
}
