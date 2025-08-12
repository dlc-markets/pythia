use super::{error::PriceFeedError, PriceFeed, Result};
use crate::data_models::{asset_pair::AssetPair, event_ids::EventId};
use awc::Client;
use chrono::{DateTime, Utc};
use log::debug;
use serde::Deserialize;
use serde_json::Value;

pub(super) struct Bitstamp {}

#[derive(Debug, Deserialize)]
struct Response {
    code: Option<String>,
    errors: Option<Vec<Value>>,
    data: Option<OhlcData>,
}

#[derive(Debug, Deserialize)]
struct OhlcData {
    ohlc: Vec<Ohlc>,
}

#[derive(Debug, Deserialize)]
struct Ohlc {
    open: String,
}

impl PriceFeed for Bitstamp {
    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        instant: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>> {
        let client = Client::new();
        let asset_pair_translation = match asset_pair {
            AssetPair::BtcUsd => "btcusd",
        };
        let start_time = instant.timestamp();

        #[derive(serde::Serialize)]
        struct BitstampQueryParams {
            step: i32,
            start: i64,
            limit: i32,
        }

        debug!("sending bitstamp http request");
        let res: Response = client
            .get(format!(
                "https://www.bitstamp.net/api/v2/ohlc/{asset_pair_translation}"
            ))
            .query(&BitstampQueryParams {
                step: 60,
                start: start_time,
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

        if let Some(errs) = res.errors {
            return Err(PriceFeedError::Server(format!(
                "bitstamp error: code {}, {:#?}",
                match res.code {
                    None => "unknown".to_string(),
                    Some(c) => c,
                },
                errs
            )));
        }

        let event_id = self.compute_event_ids(asset_pair, instant)[0];

        let response = res
            .data
            .ok_or(PriceFeedError::Server(
                "Failed to parse price from bitstamp: no data field".to_string(),
            ))?
            .ohlc
            .first()
            .ok_or(PriceFeedError::Server(
                "Failed to parse price from bitstamp: no ohlc found".to_string(),
            ))?
            .open
            .parse()
            .map_err(|e| {
                PriceFeedError::Server(format!("Failed to parse price from bitstamp: {e}"))
            })?;

        Ok(vec![(event_id, Some(response))])
    }
}
