use crate::pricefeeds::{PriceFeed, PriceFeedError, Result};
use crate::AssetPair;
use async_trait::async_trait;
use chrono::{naive::serde::ts_milliseconds, NaiveDateTime};
use chrono::{DateTime, Duration, DurationRound, TimeZone, Utc};
use log::info;
use reqwest::Client;

pub struct Lnmarkets {}

#[derive(serde::Deserialize, Debug, Clone)]
//#[serde(rename_all = "camelCase")]
pub struct LnmarketsQuote {
    #[serde(with = "ts_milliseconds")]
    pub time: NaiveDateTime,
    pub index: f64,
}

#[async_trait]
impl PriceFeed for Lnmarkets {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str {
        match asset_pair {
            AssetPair::Btcusd => "",
        }
    }

    async fn retrieve_price(&self, asset_pair: AssetPair, instant: DateTime<Utc>) -> Result<f64> {
        let client = Client::new();

        // LnMarket is only return price at minute o'clock
        let start_time = instant
            .duration_trunc(Duration::minutes(1))
            .expect("1 minute is a reasonable duration")
            .timestamp();

        info!("sending LNMarkets http request");
        let res: Vec<LnmarketsQuote> = client
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
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        if res[0].time.timestamp() != start_time {
            return Err(PriceFeedError::PriceNotAvailable(
                asset_pair,
                Utc::from_utc_datetime(&Utc, &res[0].time),
            ));
        }

        Ok(res[0].index)
    }
}
