use crate::data_models::event_ids::EventId;
use crate::data_models::{asset_pair::AssetPair, expiries::Expiry};
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::pricefeeds::{error::PriceFeedError, PriceFeed, Result};
use awc::Client;
use chrono::{DateTime, Utc};
use log::debug;

pub(super) enum Deribit {
    NoForward,
    OptionExpiries,
}

mod forward_price;
mod index_price;

impl PriceFeed for Deribit {
    fn compute_event_ids(&self, asset_pair: AssetPair, datetime: DateTime<Utc>) -> Vec<EventId> {
        match self {
            Deribit::NoForward => {
                vec![EventId::spot_from_pair_and_timestamp(asset_pair, datetime)]
            }
            Deribit::OptionExpiries => {
                let event_ids = forward_price::option_expiries_event_ids(asset_pair, datetime);
                debug!("Forward event ids at {datetime}: {event_ids:#?}");
                event_ids
            }
        }
    }

    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        instant: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>> {
        match self {
            Deribit::NoForward => index_price::retrieve_index_price(asset_pair, instant).await,
            Deribit::OptionExpiries => {
                forward_price::retrieve_option_prices(asset_pair, instant).await
            }
        }
    }
}

#[derive(serde::Deserialize, Debug, Clone)]
pub(super) struct DeribitErrorObject {
    code: i32,
    message: String,
    data: Option<String>,
}

impl std::fmt::Display for DeribitErrorObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Deribit returned an error:\n code: {},\n message: {}{}",
            self.code,
            self.message,
            self.data
                .as_ref()
                .map(|data| format!(",\ndata: {data}"))
                .unwrap_or_default()
        )
    }
}
