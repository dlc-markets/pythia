use crate::pricefeeds::{PriceFeed, PriceFeedError, Result};
use crate::AssetPair;
use async_trait::async_trait;
use chrono::{naive::serde::ts_milliseconds, NaiveDateTime};
use log::info;
use reqwest::Client;
use serde;
use time::OffsetDateTime;

pub struct Lnm {}

#[derive(serde::Deserialize, Debug, Clone)]
//#[serde(rename_all = "camelCase")]
pub struct LnmQuote {
    #[serde(with = "ts_milliseconds")]
    pub timestamp: NaiveDateTime,
    pub last_price: f64,
}

#[async_trait]
impl PriceFeed for Lnm {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::BTCUSD => "",
        }
    }

    async fn retrieve_price(&self, asset_pair: AssetPair, instant: OffsetDateTime) -> Result<f64> {
        let client = Client::new();
        let start_time = instant.unix_timestamp();
        info!("sending Lnm http request");
        let res: Vec<LnmQuote> = client
            .get("https://api.Lnmarkets.com/v1/oracle/index")
            .query(&[
                ("to", (1_000 * &start_time).to_string().as_ref()),
                ("limit", "1"),
            ])
            .send()
            .await?
            .json()
            .await?;
        info!("received response: {:#?}", res);

        if res[0].timestamp.timestamp() != start_time {
            return Err(PriceFeedError::PriceNotAvailableError(asset_pair, instant));
        }

        if res.is_empty() {
            return Err(PriceFeedError::PriceNotAvailableError(asset_pair, instant));
        }

        Ok(res[0].last_price)
    }
}
