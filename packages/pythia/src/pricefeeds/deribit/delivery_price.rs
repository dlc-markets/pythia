///// Helper function to get the delivery price for an expiry /////

use awc::Client;
use chrono::{DateTime, Timelike, Utc};
use serde::Deserialize;

use super::{DeribitErrorObject, Result};

use crate::{data_models::asset_pair::AssetPair, pricefeeds::error::PriceFeedError};

const DELIVERY_PRICES: &str = "https://www.deribit.com/api/v2/public/get_delivery_prices";

/// Check if the given date is an expiry date.
pub(super) fn is_an_expiry_date(date: DateTime<Utc>) -> bool {
    // All expiries are every day at 8:00:00 UTC.
    date.hour() == 8 && date.minute() == 0 && date.second() == 0
}

pub async fn retrieve_delivery_price(
    client: &Client,
    asset_pair: AssetPair,
    instant: DateTime<Utc>,
) -> Result<f64> {
    if is_an_expiry_date(instant) {
        Ok(get_delivery_price_for_expiry(client, asset_pair, instant).await?)
    } else {
        Err(PriceFeedError::PriceNotAvailable(asset_pair, instant))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DeribitDeliveryPriceResponse {
    ResultResponse { result: DeribitDeliveryPriceData },
    ErrorResponse { error: DeribitErrorObject },
}

#[derive(Debug, Deserialize)]
struct DeribitDeliveryPriceData {
    data: Vec<DeribitDeliveryPrice>,
}

#[derive(Debug, Deserialize)]
struct DeribitDeliveryPrice {
    delivery_price: f64,
    date: String,
}

/// Query the Deribit API to get the delivery price for the given expiry date and index.
/// Expects `expiry` to be an expiry date.
async fn get_delivery_price_for_expiry(
    client: &Client,
    asset_pair: AssetPair,
    expiry: DateTime<Utc>,
) -> Result<f64> {
    let asset_pair_translation = match asset_pair {
        AssetPair::BtcUsd => "btc_usd",
    };

    // We need to offset the request by the number of days between the expiry and the present
    // to get the delivery price for the given expiry.
    let days = (Utc::now() - expiry).num_days().to_string();

    // Make the API request
    let response = match client
        .get(DELIVERY_PRICES)
        .query(&[
            ("index_name", asset_pair_translation),
            ("count", "1"),
            ("offset", &days),
        ])
        .expect("can be serialized")
        .send()
        .await
        .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?
        .json::<DeribitDeliveryPriceResponse>()
        .await
        .map_err(|e| PriceFeedError::ConnectionError(e.to_string()))?
    {
        DeribitDeliveryPriceResponse::ResultResponse { result } => result,
        DeribitDeliveryPriceResponse::ErrorResponse { error } => {
            return Err(PriceFeedError::Server(error.to_string()));
        }
    };

    debug!("Got this delivery price response from deribit: {response:#?}",);

    // Extract the price while checking it corresponds to the asked date.
    response
        .data
        .first()
        .and_then(|price| {
            (price.date == expiry.format("%Y-%m-%d").to_string()).then_some(price.delivery_price)
        })
        .ok_or(PriceFeedError::PriceNotAvailable(asset_pair, expiry))
}
