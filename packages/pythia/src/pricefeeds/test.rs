use chrono::{SubsecRound, Utc};

use crate::data_models::asset_pair::AssetPair;

use super::{error::PriceFeedError, ImplementedPriceFeed};
use strum::IntoEnumIterator;

// Test all the implemented pricefeeds. Failing mean there has been breaking change in a pricefeed API
#[actix_web::test]
async fn test_all_pricefeeds() {
    let mut deprecated: Vec<(ImplementedPriceFeed, PriceFeedError)> = vec![];
    let now = Utc::now().trunc_subsecs(0);

    for pricefeed in ImplementedPriceFeed::iter() {
        let _ = pricefeed
            .retrieve_prices(AssetPair::BtcUsd, now)
            .await
            .map_err(|e| deprecated.push((pricefeed, e)));
    }
    if !deprecated.is_empty() {
        panic!("Some pricefeed APIs seem deprecated: {deprecated:?}\n No answer for date {now}")
    }
}
