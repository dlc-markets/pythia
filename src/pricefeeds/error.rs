use crate::AssetPair;
use chrono::{DateTime, Utc};
use displaydoc::Display;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, PriceFeedError>;

#[derive(Debug, Display, Error)]
pub enum PriceFeedError {
    /// internal error: {0}
    InternalError(String),

    /// price not available for {0} at {1}
    PriceNotAvailableError(AssetPair, DateTime<Utc>),

    /// http error: {0}
    HttpError(#[from] reqwest::Error),
}
