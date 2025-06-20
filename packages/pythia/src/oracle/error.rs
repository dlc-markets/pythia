use displaydoc::Display;
use thiserror::Error;

use crate::{data_models::event_ids::EventId, pricefeeds::error::PriceFeedError};

pub type Result<T> = std::result::Result<T, OracleError>;

#[derive(Debug, Display, Error)]
pub(crate) enum OracleError {
    /// database error: {0}
    DatabaseError(#[from] sqlx::Error),

    /// Some requested announcement are missing
    MissingAnnouncements,

    /// pricefeed error: {0}
    PriceFeedError(#[from] PriceFeedError),

    /// Could not attest Event Id {0}
    MissingEventId(EventId),
}
