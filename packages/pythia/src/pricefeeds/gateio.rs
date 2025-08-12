use super::{error::PriceFeedError, PriceFeed, Result};
use crate::data_models::{asset_pair::AssetPair, event_ids::EventId};
use awc::Client;
use chrono::{DateTime, Utc};
use log::debug;
use serde_json::Value;

pub(super) struct GateIo {}

impl PriceFeed for GateIo {
    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        instant: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>> {
        let client = Client::new();
        let start_time = instant.timestamp();

        let asset_pair_translation = match asset_pair {
            AssetPair::BtcUsd => "BTC_USDT",
        };

        #[derive(serde::Serialize)]
        struct GateIoQueryParams {
            currency_pair: &'static str,
            from: i64,
            limit: i32,
        }

        debug!("sending gate.io http request");
        let res: Vec<Vec<Value>> = client
            .get("https://api.gateio.ws/api/v4/spot/candlesticks")
            .query(&GateIoQueryParams {
                currency_pair: asset_pair_translation,
                from: start_time,
                limit: 1,
            })
            .expect("can be serialized")
            .send()
            .await
            .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?
            .json()
            .await
            .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?;
        debug!("received response: {res:#?}");

        if res.is_empty() {
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        let event_id = self.compute_event_ids(asset_pair, instant)[0];

        let response = res[0][5]
            .as_str()
            .ok_or(PriceFeedError::Server(format!(
                "Failed to parse price from gate.io: expect a string, got {:#?}",
                res[0][5]
            )))?
            .parse()
            .map_err(|e| {
                PriceFeedError::Server(format!("Failed to parse price from gate.io: {e}"))
            })?;

        Ok(vec![(event_id, Some(response))])
    }
}
