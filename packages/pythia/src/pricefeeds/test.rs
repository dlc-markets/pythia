use chrono::{SubsecRound, Utc};
use futures_buffered::IterExt as _;

use crate::data_models::{asset_pair::AssetPair, event_ids::EventIdInfos};

use super::ImplementedPriceFeed;
use strum::IntoEnumIterator;

// Test all the implemented pricefeeds. Failing mean there has been breaking change in a pricefeed API
#[actix_web::test]
async fn test_all_pricefeeds() {
    let now = Utc::now().trunc_subsecs(0);

    let results = ImplementedPriceFeed::iter()
        .map(async |pricefeed| {
            if let Err(e) = pricefeed
                .retrieve_prices(vec![EventIdInfos::spot_from_pair_and_timestamp(
                    AssetPair::BtcUsd,
                    now,
                )])
                .await
            {
                Some((pricefeed, e))
            } else {
                None
            }
        })
        .join_all()
        .await;

    let deprecated = results
        .into_iter()
        .filter_map(|result| result)
        .collect::<Vec<_>>();
    if !deprecated.is_empty() {
        panic!("Some pricefeed APIs seem deprecated: {deprecated:?}\n No answer for date {now}")
    }
}
