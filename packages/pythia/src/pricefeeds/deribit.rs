use crate::data_models::asset_pair::AssetPair;
use crate::data_models::event_ids::EventId;
use crate::pricefeeds::{error::PriceFeedError, PriceFeed, Result};
use chrono::{DateTime, Utc};
use log::debug;
use reqwest::Client;

pub(super) struct Deribit {}

#[derive(serde::Deserialize, Debug, Clone)]

struct DeribitQuote {
    index_price: f64,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct DeribitResponse {
    result: DeribitQuote,
    us_in: u64,
}

impl PriceFeed for Deribit {
    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        instant: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>> {
        let client = Client::new();
        let start_time = instant.timestamp();

        let now = Utc::now().timestamp();
        if now - start_time > 60 {
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        let asset_pair_translation = match asset_pair {
            AssetPair::BtcUsd => "btc_usd",
        };

        debug!("sending Deribit http request");
        let res: DeribitResponse = client
            .get("https://www.deribit.com/api/v2/public/get_index_price")
            .query(&[("index_name", asset_pair_translation)])
            .send()
            .await?
            .json()
            .await?;
        debug!("received response: {:#?}", res);

        // Deribit does not allow to retrieve past index price
        // So we check that we are not asking for price more than a minute ago
        // if we do then we return that price is not available to not attest anything
        // A fallback pricefeed can be used instead in the future

        if res.us_in / 1_000_000 - start_time as u64 > 60 {
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        let event_id = self.compute_event_ids(asset_pair, instant)[0];

        Ok(vec![(event_id, Some(res.result.index_price))])
    }
}
