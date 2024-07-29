use super::{error::PriceFeedError, PriceFeed, Result};
use crate::AssetPair;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::info;
use reqwest::Client;
use serde_json::Value;

pub(super) struct GateIo {}

#[async_trait]
impl PriceFeed for GateIo {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::BtcUsd => "BTC_USDT",
        }
    }

    async fn retrieve_price(&self, asset_pair: AssetPair, instant: DateTime<Utc>) -> Result<f64> {
        let client = Client::new();
        let start_time = instant.timestamp();
        debug!("sending gateio http request");
        let res: Vec<Vec<Value>> = client
            .get("https://api.gateio.ws/api/v4/spot/candlesticks")
            .query(&[
                ("currency_pair", self.translate_asset_pair(asset_pair)),
                ("from", &start_time.to_string()),
                ("limit", "1"),
            ])
            .send()
            .await?
            .json()
            .await?;
        debug!("received response: {:#?}", res);

        if res.is_empty() {
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        Ok(res[0][5].as_str().unwrap().parse().unwrap())
    }
}
