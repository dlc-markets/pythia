use super::{PriceFeed, Result, error::PriceFeedError};
use crate::{
    data_models::{asset_pair::AssetPair, event_ids::EventIdInfos},
    pricefeeds::HTTP_CLIENT,
};
use futures::TryStreamExt as _;
use futures_buffered::FuturesOrderedBounded;
use log::debug;
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

                let asset_pair_translation = match asset_pair {
                    AssetPair::BtcUsd => "XXBTZUSD",
                };
                let start_time = maturation.timestamp();

                #[derive(serde::Serialize)]
                struct KrakenQueryParams {
                    pair: &'static str,
                    since: i64,
                }

                debug!("sending kraken http request");
                let res: Response = HTTP_CLIENT
                    .with(|client| {
                        client
                            .get("https://api.kraken.com/0/public/OHLC")
                            .query(&KrakenQueryParams {
                                pair: asset_pair_translation,
                                since: start_time,
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

                if !res.error.is_empty() {
                    return Err(PriceFeedError::Server(format!(
                        "kraken error: {:#?}",
                        res.error
                    )));
                }

                let res = res
                    .result
                    .get(asset_pair_translation)
                    .ok_or(PriceFeedError::PriceNotAvailable(asset_pair, maturation))?;

                Ok((
                    event_id_info,
                    Some(
                        res[0][1]
                            .as_str()
                            .ok_or(PriceFeedError::Server(format!(
                                "Failed to parse price from kraken: expect a string, got {:#?}",
                                res[0][1]
                            )))?
                            .parse()
                            .map_err(|e| {
                                PriceFeedError::Server(format!(
                                    "Failed to parse price from kraken: {e}"
                                ))
                            })?,
                    ),
                ))
            })
            .collect::<FuturesOrderedBounded<_>>()
            .try_collect()
            .await
    }
}
