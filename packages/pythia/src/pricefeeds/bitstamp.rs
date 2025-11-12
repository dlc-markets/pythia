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
                    AssetPair::BtcUsd => "btcusd",
                };
                let start_time = maturation.timestamp();

                #[derive(serde::Serialize)]
                struct BitstampQueryParams {
                    step: i32,
                    start: i64,
                    limit: i32,
                }

                debug!("sending bitstamp http request");
                let res: Response = HTTP_CLIENT
                    .with(|client| {
                        client
                            .get(format!(
                                "https://www.bitstamp.net/api/v2/ohlc/{asset_pair_translation}"
                            ))
                            .query(&BitstampQueryParams {
                                step: 60,
                                start: start_time,
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

                Ok((
                    event_id_info,
                    Some(
                        res.data
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
                                PriceFeedError::Server(format!(
                                    "Failed to parse price from bitstamp: {e}"
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
