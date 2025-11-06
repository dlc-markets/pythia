use crate::data_models::asset_pair::AssetPair;
use crate::data_models::event_ids::EventIdInfos;
use crate::pricefeeds::HTTP_CLIENT;
use crate::pricefeeds::{PriceFeed, Result, error::PriceFeedError};
use futures::TryStreamExt as _;
use futures_buffered::FuturesOrderedBounded;
use log::debug;

pub(super) struct Deribit {}

#[derive(serde::Deserialize, Debug, Clone)]

struct DeribitQuote {
    index_price: f64,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct DeribitResponse {
    result: DeribitQuote,
    us_in: u64,
}

impl PriceFeed for Deribit {
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
                    AssetPair::BtcUsd => "btc_usd",
                };
                let start_time = maturation.timestamp();
                #[derive(serde::Serialize)]
                struct DeribitQueryParams {
                    index_name: &'static str,
                }

                debug!("sending Deribit http request");
                let res: DeribitResponse = HTTP_CLIENT
                    .with(|client| {
                        client
                            .get("https://www.deribit.com/api/v2/public/get_index_price")
                            .query(&DeribitQueryParams {
                                index_name: asset_pair_translation,
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

                // Deribit does not allow to retrieve past index price
                // So we check that we are not asking for price more than a minute ago
                // if we do then we return that price is not available to not attest anything
                // A fallback pricefeed can be used instead in the future

                if res.us_in / 1_000_000 - start_time as u64 > 60 {
                    return Err(PriceFeedError::PriceNotAvailable(asset_pair, maturation));
                }

                Ok((event_id_info, Some(res.result.index_price)))
            })
            .collect::<FuturesOrderedBounded<_>>()
            .try_collect()
            .await
    }
}
