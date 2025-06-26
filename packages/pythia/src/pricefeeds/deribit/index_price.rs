use super::Result;
use crate::{
    data_models::{asset_pair::AssetPair, event_ids::EventId},
    pricefeeds::error::PriceFeedError,
};
use chrono::{DateTime, TimeDelta, Utc};
use log::debug;
use reqwest::Client;

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct DeribitResponseIndex {
    result: DeribitQuote,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct DeribitQuote {
    index_price: f64,
}

pub async fn retrieve_index_price(
    asset_pair: AssetPair,
    instant: DateTime<Utc>,
) -> Result<Vec<(EventId, Option<f64>)>> {
    let client = Client::new();
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
        .get("https://www.deribit.com/api/v2/public/get_index_price")
        .query(&[("index_name", asset_pair_translation)])
        .send()
        .await?
        .json::<DeribitResponseIndex>()
        .await?;
    debug!("received response: {:#?}", res);

    let event_id = EventId::spot_from_pair_and_timestamp(asset_pair, instant);

    Ok(vec![(event_id, Some(res.result.index_price))])
}
