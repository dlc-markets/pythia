use crate::pricefeeds::{PriceFeed, PriceFeedError, Result};
use crate::AssetPair;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::info;
use reqwest::Client;

pub struct Deribit {}

#[derive(serde::Deserialize, Debug, Clone)]

pub struct DeribitQuote {
    pub index_price: f64,
    pub estimated_delivery_price: f64,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeribitResponse {
    pub jsonrpc: String,
    pub result: DeribitQuote,
    pub us_in: u64,
    pub us_out: u64,
    pub us_diff: u64,
    pub testnet: bool,
}

#[async_trait]
impl PriceFeed for Deribit {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::Btcusd => "btc_usd",
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
        info!("sending Deribit http request");
        let res: DeribitResponse = client
            .get("https://www.deribit.com/api/v2/public/get_index_price")
            .query(&[("index_name", asset_pair_translation)])
            .send()
            .await?
            .json()
            .await?;
        info!("received response: {:#?}", res);

        // Deribit does not allow to retrieve past index price
        // So we check that we are not asking for price more than a minute ago
        // if we do then we return that price is not available to not attest anything
        // A fallback pricefeeder can be used instead in the future

        if res.us_in / 1_000_000 - start_time as u64 > 60 {
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        Ok(res.result.index_price)
    }
}
