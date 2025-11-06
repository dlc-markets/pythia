use crate::data_models::asset_pair::AssetPair;
use crate::data_models::event_ids::EventIdInfos;
use crate::pricefeeds::HTTP_CLIENT;
use crate::pricefeeds::{PriceFeed, Result, error::PriceFeedError};
use chrono::{Duration, DurationRound, TimeZone, Utc};
use chrono::{NaiveDateTime, naive::serde::ts_milliseconds};

use futures_buffered::IterExt;
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
        event_ids_infos: impl IntoIterator<Item = EventIdInfos>,
    ) -> Result<Vec<(EventIdInfos, Option<f64>)>> {
        event_ids_infos
            .into_iter()
            .map(async |event_id_info| {
                let EventIdInfos {
                    asset_pair,
                    maturation,
                    ..
                } = event_id_info;

                if !event_id_info.is_spot() || asset_pair != AssetPair::BtcUsd {
                    return Ok((event_id_info, None));
                };

                // LnMarket is only return price at minute o'clock
                let start_time = maturation
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
                let res: Vec<LnmarketsQuote> = HTTP_CLIENT
                    .with(|client| {
                        client
                            .get("https://api.Lnmarkets.com/v2/oracle/index")
                            .insert_header(("User-Agent", "Actix-web"))
                            .query(&LnmarketsQueryParams {
                                to: (1_000 * start_time),
                                from: (1_000 * start_time),
                                limit: 1,
                            })
                            .expect("can be serialized")
                    })
                    .send()
                    .await
                    .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?
                    .json()
                    .await
                    .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?;
                debug!("received response: {res:#?}");

                if res.is_empty() {
                    return Ok((event_id_info, None));
                }

                if res[0].time.and_utc().timestamp() != start_time {
                    return Err(PriceFeedError::PriceNotAvailable(
                        asset_pair,
                        Utc::from_utc_datetime(&Utc, &res[0].time),
                    ));
                }

                Ok((event_id_info, Some(res[0].index)))
            })
            .try_join_all()
            .await
    }
}
