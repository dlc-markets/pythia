use super::Result;
use crate::{
    data_models::asset_pair::AssetPair,
    pricefeeds::{deribit::DeribitErrorObject, error::PriceFeedError},
};
use awc::Client;
use chrono::{DateTime, TimeDelta, Utc};
use log::debug;

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
enum DeribitResponseIndex {
    ResultResponse { result: DeribitQuote },
    ErrorResponse { error: DeribitErrorObject },
}

#[derive(serde::Deserialize, Debug, Clone)]
struct DeribitQuote {
    index_price: f64,
}

const INDEX_PRICE_URL: &str = "https://www.deribit.com/api/v2/public/get_index_price";

pub async fn retrieve_index_price(
    client: &Client,
    asset_pair: AssetPair,
    instant: DateTime<Utc>,
) -> Result<f64> {
    let asset_pair_translation = match asset_pair {
        AssetPair::BtcUsd => "btc_usd",
    };

    if Utc::now() - instant > TimeDelta::minutes(1) {
        info!(
            "Requested attesting data from the past with deribit pricefeed. Skipping index price."
        );
        return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
    }

    debug!("sending Deribit http request");
    let res = client
        .get(INDEX_PRICE_URL)
        .query(&[("index_name", asset_pair_translation)])
        .expect("can be serialized")
        .send()
        .await
        .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?
        .json::<DeribitResponseIndex>()
        .await
        .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?;

    let quote = match res {
        DeribitResponseIndex::ResultResponse { result } => result.index_price,
        DeribitResponseIndex::ErrorResponse { error } => {
            return Err(PriceFeedError::Server(error.to_string()));
        }
    };

    Ok(quote)
}
