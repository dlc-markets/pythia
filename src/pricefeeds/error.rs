use crate::AssetPair;
use chrono::{DateTime, Utc};
use displaydoc::Display;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, PriceFeedError>;

#[derive(Debug, Display, Error)]
pub enum PriceFeedError {
    /// server internal error: {0}
    Server(String),

    /// price not available for {0} at {1}
    PriceNotAvailable(AssetPair, DateTime<Utc>),

    /// http request error: {0}
    HttpRequest(#[from] reqwest::Error),
}
