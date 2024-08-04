use crate::pricefeeds::{error::PriceFeedError, PriceFeed, Result};
use crate::AssetPair;
use async_trait::async_trait;
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

#[async_trait]
impl PriceFeed for Deribit {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::BtcUsd => "btc_usd",
        }
    }

    async fn retrieve_price(&self, asset_pair: AssetPair, instant: DateTime<Utc>) -> Result<f64> {
        let client = Client::new();
        let start_time = instant.timestamp();

        let now = Utc::now().timestamp();
        if now - start_time > 60 {
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        let asset_pair_translation = self.translate_asset_pair(asset_pair);
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

        Ok(res.result.index_price)
    }
}
