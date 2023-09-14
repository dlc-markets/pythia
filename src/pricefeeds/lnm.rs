use crate::pricefeeds::{PriceFeed, PriceFeedError, Result};
use crate::AssetPair;
use async_trait::async_trait;
use chrono::{naive::serde::ts_milliseconds, NaiveDateTime};
use log::info;
use reqwest::Client;
use time::OffsetDateTime;

pub struct Lnm {}

#[derive(serde::Deserialize, Debug, Clone)]
//#[serde(rename_all = "camelCase")]
pub struct LnmQuote {
    #[serde(with = "ts_milliseconds")]
    pub time: NaiveDateTime,
    pub index: f64,
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
            .get("https://api.Lnmarkets.com/v2/oracle/index")
            .query(&[
                ("to", (1_000 * &start_time).to_string().as_ref()),
                ("from", (1_000 * &start_time).to_string().as_ref()),
                ("limit", "1"),
            ])
            .send()
            .await?
            .json()
            .await?;
        info!("received response: {:#?}", res);

        if res.is_empty() {
            return Err(PriceFeedError::PriceNotAvailableError(asset_pair, instant));
        }

        if res[0].time.timestamp() != start_time {
            return Err(PriceFeedError::PriceNotAvailableError(asset_pair, instant));
        }

        Ok(res[0].index)
    }
}
