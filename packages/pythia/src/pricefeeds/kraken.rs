use super::{error::PriceFeedError, PriceFeed, Result};
use crate::data_models::{asset_pair::AssetPair, event_ids::EventId};
use chrono::{DateTime, Utc};
use log::debug;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

pub(super) struct Kraken {}

#[derive(Debug, Deserialize)]
struct Response {
    error: Vec<String>,
    result: HashMap<String, Value>,
}

impl PriceFeed for Kraken {
    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        instant: DateTime<Utc>,
    ) -> Result<Vec<(EventId, f64)>> {
        let client = Client::new();
        let asset_pair_translation = match asset_pair {
            AssetPair::BtcUsd => "XXBTZUSD",
        };
        let start_time = instant.timestamp();
        debug!("sending kraken http request");
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
        debug!("received response: {:#?}", res);

        if !res.error.is_empty() {
            return Err(PriceFeedError::Server(format!(
                "kraken error: {:#?}",
                res.error
            )));
        }

        let event_id = self.compute_event_ids(asset_pair, instant)[0];

        let res = res
            .result
            .get(asset_pair_translation)
            .ok_or(PriceFeedError::PriceNotAvailable(asset_pair, instant))?;

        let response = res[0][1]
            .as_str()
            .ok_or(PriceFeedError::Server(format!(
                "Failed to parse price from kraken: expect a string, got {:#?}",
                res[0][1]
            )))?
            .parse()
            .map_err(|e| {
                PriceFeedError::Server(format!("Failed to parse price from kraken: {e}"))
            })?;

        Ok(vec![(event_id, response)])
    }
}
