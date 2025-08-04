use std::collections::BTreeMap;

use super::Result;
use crate::{data_models::asset_pair::AssetPair, pricefeeds::error::PriceFeedError};
use chrono::{DateTime, TimeDelta, Utc};
use log::debug;
use reqwest::Client;

pub(super) const BOOK_SUMMARY_BY_CURRENCY: &str =
    "https://www.deribit.com/api/v2/public/get_book_summary_by_currency";

/// Helper functions module to compute event ids and get the quoting expiries.
mod quoting_expiries;
use quoting_expiries::iter_all_quoting_options_forward_at_date;

pub fn option_expiries(datetime: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    iter_all_quoting_options_forward_at_date(datetime)
        .into_iter()
        .collect::<Vec<_>>()
}

pub async fn retrieve_dated_option_prices(
    client: &Client,
    asset_pair: AssetPair,
    instant: DateTime<Utc>,
) -> Result<BTreeMap<DateTime<Utc>, f64>> {
    if Utc::now() - instant > TimeDelta::minutes(1) {
        // The book summary only allow to get current view of the market, so we can't attest
        // forward prices that are in the past.
        info!("Requested attesting data from the past with deribit pricefeed. Only past delivery prices can be attested");

        return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
    }

    let currency = match asset_pair {
        AssetPair::BtcUsd => "BTC",
    };

    let response_payload = client
        .get(BOOK_SUMMARY_BY_CURRENCY)
        .query(&[("kind", "future"), ("currency", currency)])
        .send()
        .await?
        .json::<DeribitResponseForward>()
        .await?;

    let expiry_map = match response_payload {
        DeribitResponseForward::ErrorResponse { error } => {
            warn!("{error}");
            None
        }
        DeribitResponseForward::ResultResponse { result } => {
            debug!("Got these futures prices from deribit: {result:#?}");
            Some(result)
        }
    }
    .ok_or(PriceFeedError::PriceNotAvailable(asset_pair, instant))?;

    debug!("Returning these prices with expiries: {expiry_map:?}",);

    Ok(expiry_map)
}

///// Helper function to deserialize the Deribit book summary response /////

use super::DeribitErrorObject;

#[derive(Debug, Clone)]
enum DeribitResponseForward {
    ResultResponse {
        result: BTreeMap<DateTime<Utc>, f64>,
    },
    ErrorResponse {
        error: DeribitErrorObject,
    },
}

// Deserialize implementation is in the `custom_serde.rs` file.
mod custom_serde;

#[cfg(test)]
pub use quoting_expiries::futures_quotation::iter_all_quoting_futures_forward_at_date;
