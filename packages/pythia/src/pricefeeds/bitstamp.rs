use super::{error::PriceFeedError, PriceFeed, Result};
use crate::AssetPair;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::debug;
use reqwest::Client;
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

#[async_trait]
impl PriceFeed for Bitstamp {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::BtcUsd => "btcusd",
        }
    }

    async fn retrieve_price(&self, asset_pair: AssetPair, instant: DateTime<Utc>) -> Result<f64> {
        let client = Client::new();
        let asset_pair_translation = self.translate_asset_pair(asset_pair);
        let start_time = instant.timestamp();
        debug!("sending bitstamp http request");
        let res: Response = client
            .get(format!(
                "https://www.bitstamp.net/api/v2/ohlc/{}",
                asset_pair_translation
            ))
            .query(&[
                ("step", "60"),
                ("start", &start_time.to_string()),
                ("limit", "1"),
            ])
            .send()
            .await?
            .json()
            .await?;
        debug!("received response: {:#?}", res);

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

        Ok(res.data.unwrap().ohlc[0].open.parse().unwrap())
    }
}
