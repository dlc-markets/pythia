use super::{PriceFeed, PriceFeedError, Result};
use crate::AssetPair;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::info;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

pub struct Kraken {}

#[derive(Debug, Deserialize)]
struct Response {
    error: Vec<String>,
    result: HashMap<String, Value>,
}

#[async_trait]
impl PriceFeed for Kraken {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::Btcusd => "XXBTZUSD",
        }
    }

    async fn retrieve_price(&self, asset_pair: AssetPair, instant: DateTime<Utc>) -> Result<f64> {
        let client = Client::new();
        let asset_pair_translation = self.translate_asset_pair(asset_pair);
        let start_time = instant.timestamp();
        info!("sending kraken http request");
        let res: Response = client
            .get("https://api.kraken.com/0/public/OHLC")
            .query(&[
                ("pair", asset_pair_translation),
                ("since", &start_time.to_string()),
            ])
            .send()
            .await?
            .json()
            .await?;
        info!("received response: {:#?}", res);

        if !res.error.is_empty() {
            return Err(PriceFeedError::InternalError(format!(
                "kraken error: {:#?}",
                res.error
            )));
        }

        let res = res
            .result
            .get(asset_pair_translation)
            .ok_or(PriceFeedError::PriceNotAvailableError(asset_pair, instant))?;

        Ok(res[0][1].as_str().unwrap().parse().unwrap())
    }
}
