use super::{PriceFeed, Result, error::PriceFeedError};
use crate::{
    data_models::{asset_pair::AssetPair, event_ids::EventIdInfos},
    pricefeeds::HTTP_CLIENT,
};
use futures::TryStreamExt as _;
use futures_buffered::FuturesOrderedBounded;
use log::debug;
use serde_json::Value;

pub(super) struct GateIo {}

impl PriceFeed for GateIo {
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
                    AssetPair::BtcUsd => "BTC_USDT",
                };
                let start_time = maturation.timestamp();

                #[derive(serde::Serialize)]
                struct GateIoQueryParams {
                    currency_pair: &'static str,
                    from: i64,
                    limit: i32,
                }

                debug!("sending gate.io http request");
                let res: Vec<Vec<Value>> = HTTP_CLIENT
                    .with(|client| {
                        client
                            .get("https://api.gateio.ws/api/v4/spot/candlesticks")
                            .query(&GateIoQueryParams {
                                currency_pair: asset_pair_translation,
                                from: start_time,
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
                    return Err(PriceFeedError::PriceNotAvailable(asset_pair, maturation));
                }

                Ok((
                    event_id_info,
                    Some(
                        res[0][5]
                            .as_str()
                            .ok_or(PriceFeedError::Server(format!(
                                "Failed to parse price from gate.io: expect a string, got {:#?}",
                                res[0][5]
                            )))?
                            .parse()
                            .map_err(|e| {
                                PriceFeedError::Server(format!(
                                    "Failed to parse price from gate.io: {e}"
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
