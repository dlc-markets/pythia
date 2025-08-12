use crate::data_models::{asset_pair::AssetPair, event_ids::EventId};
use crate::pricefeeds::{error::PriceFeedError, PriceFeed, Result};
use awc::Client;
use chrono::{naive::serde::ts_milliseconds, NaiveDateTime};
use chrono::{DateTime, Duration, DurationRound, TimeZone, Utc};
use log::debug;

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

        #[derive(serde::Serialize)]
        struct LnmarketsQueryParams {
            to: i64,
            from: i64,
            limit: i32,
        }

        debug!("sending LNMarkets http request");
        let res: Vec<LnmarketsQuote> = client
            .get("https://api.Lnmarkets.com/v2/oracle/index")
            .insert_header(("User-Agent", "Actix-web"))
            .query(&LnmarketsQueryParams {
                to: (1_000 * start_time),
                from: (1_000 * start_time),
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

        if res[0].time.and_utc().timestamp() != start_time {
            return Err(PriceFeedError::PriceNotAvailable(
                asset_pair,
                Utc::from_utc_datetime(&Utc, &res[0].time),
            ));
        }

        Ok(vec![(event_id, Some(res[0].index))])
    }
}
