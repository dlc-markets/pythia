use crate::data_models::{asset_pair::AssetPair, event_ids::EventId};
use crate::pricefeeds::{error::PriceFeedError, PriceFeed, Result};
use chrono::{naive::serde::ts_milliseconds, NaiveDateTime};
use chrono::{DateTime, Duration, DurationRound, TimeZone, Utc};
use log::debug;
use reqwest::Client;

pub(super) struct Lnmarkets {}

#[derive(serde::Deserialize, Debug, Clone)]
//#[serde(rename_all = "camelCase")]
struct LnmarketsQuote {
    #[serde(with = "ts_milliseconds")]
    pub time: NaiveDateTime,
    pub index: f64,
}

impl PriceFeed for Lnmarkets {
    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        instant: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>> {
        let client = Client::new();

        // LnMarket is only return price at minute o'clock
        let start_time = instant
            .duration_trunc(Duration::minutes(1))
            .expect("1 minute is a reasonable duration")
            .timestamp();

        debug!("sending LNMarkets http request");
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
        debug!("received response: {:#?}", res);

        if res.is_empty() {
            return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
        }

        let event_id = self.compute_event_ids(asset_pair, instant)[0];

        if res[0].time.and_utc().timestamp() != start_time {
            return Err(PriceFeedError::PriceNotAvailable(
                asset_pair,
                Utc::from_utc_datetime(&Utc, &res[0].time),
            ));
        }

        Ok(vec![(event_id, Some(res[0].index))])
    }
}
