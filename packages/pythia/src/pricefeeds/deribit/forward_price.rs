use std::collections::HashMap;

use super::Result;
use crate::{
    data_models::{asset_pair::AssetPair, event_ids::EventId, expiries::Expiry},
    pricefeeds::error::PriceFeedError,
};
use chrono::{DateTime, TimeDelta, Utc};
use log::debug;
use reqwest::Client;

const BOOK_SUMMARY_BY_CURRENCY: &str =
    "https://www.deribit.com/api/v2/public/get_book_summary_by_currency";

const DELIVERY_PRICES: &str = "https://www.deribit.com/api/v2/public/get_delivery_prices";

#[cfg(test)]
mod test;

/// Helper functions module to compute event ids and get the quoting expiries.
mod quoting_expiries;
use quoting_expiries::{get_all_quoting_forward_at_date, is_an_expiry_date};

pub fn option_expiries_event_ids(asset_pair: AssetPair, datetime: DateTime<Utc>) -> Vec<EventId> {
    let mut event_ids = is_an_expiry_date(datetime)
        .then_some(EventId::delivery_of_expiry_with_pair(
            asset_pair,
            Expiry::from(datetime),
        ))
        .into_iter()
        .chain(get_all_quoting_forward_at_date(datetime).map(|expiry| {
            EventId::forward_of_expiry_with_pair_at_timestamp(
                asset_pair,
                Expiry::from(expiry),
                datetime,
            )
        }))
        .collect::<Vec<_>>();

    event_ids.sort_unstable();
    event_ids.dedup();

    event_ids
}

pub async fn retrieve_option_prices(
    asset_pair: AssetPair,
    instant: DateTime<Utc>,
) -> Result<Vec<(EventId, Option<f64>)>> {
    let client = Client::new();

    let mut expiries = get_all_quoting_forward_at_date(instant).collect::<Vec<_>>();
    expiries.sort_unstable();
    expiries.dedup();
    let nb_expiries = expiries.len();
    let closest_expiry = expiries[0];

    let response_payload = if Utc::now() - instant > TimeDelta::minutes(1) {
        // The book summary only allow to get current view of the market, so we can't attest
        // forward prices that are in the past.
        info!("Requested attesting data from the past with deribit pricefeed. Only past delivery prices can be attested");
        None
    } else {
        let currency = match asset_pair {
            AssetPair::BtcUsd => "BTC",
        };

        // We use the book summary of options to get underlying prices for each expiry, even though this returns redundant data,
        // because the futures book endpoint does not return the index price. The index price is needed to compute synthetic
        // underlying prices for option expiries that don't have tradeable futures. Without the index price from the futures endpoint,
        // we would need a separate request to get the current index value to calculate missing futures prices. This introduces the
        // risk of inconsistent data if the market moves significantly between the two requests. We prefer making one larger request
        // that returns redundant data (when multiple options share the same expiry) rather than risk inconsistent prices from
        // multiple smaller requests.
        if let Ok(result) = client
            .get(BOOK_SUMMARY_BY_CURRENCY)
            .query(&[("kind", "option"), ("currency", currency)])
            .send()
            .await
        {
            result.bytes().await.ok()
        } else {
            None
        }
    };

    let mut deribit_response = response_payload
        .as_ref()
        .and_then(|payload| deserialize_deribit_response_forward(payload, nb_expiries));

    // According to https://support.deribit.com/hc/en-us/articles/25944782980253-25-June-2021,
    // when we query the book summary by currency and an expiry is close to the current date,
    // the underlying price returned by Deribit is changed for the estimated delivery price and
    // the underlying index is renamed "SYN.EXPIRY". However, the deserialization of the response
    // will remove the "SYN." prefix and the 4 next chars from the index name. This means that if
    // Deribit is returning a "SYN.EXPIRY" underlying index, it will appear in our hashmap as just
    // "RY" and we know the matching price is the underlying price of options that will expire very
    // soon. This means the associated expiry is the closest to present from our list of quoting
    // expiries. Conclusion: we must remove this entry from the hashmap and use one with the
    // closest expiry from our list of quoting expiries (to get consistent event_id) and the same
    // underlying_price. This is done in the `maybe_close_expiry` match statement.
    let maybe_close_to_expiry_synthetic_price =
        deribit_response.as_mut().and_then(|r| r.remove("RY"));

    let maybe_close_expiry_entry = match maybe_close_to_expiry_synthetic_price {
        Some(price) => Some((
            if is_an_expiry_date(instant) {
                // At expiry, "SYN.EXPIRY" will exactly matches the delivery price for a short time.
                EventId::delivery_of_expiry_with_pair(asset_pair, Expiry::from(instant))
            } else {
                EventId::forward_of_expiry_with_pair_at_timestamp(
                    asset_pair,
                    Expiry::from(closest_expiry),
                    instant,
                )
            },
            Some(price),
        )),
        // If we are attesting for an expiry date, possibly in the past, we try to get the delivery price
        // for the expiry through the delivery prices endpoint if we miss it in the current book summary.
        // This allows to attest the delivery price even after the fact, which is often
        // the most important price that must be attested for a DLC contract using forward price data.
        None if is_an_expiry_date(instant) => Some((
            EventId::delivery_of_expiry_with_pair(asset_pair, Expiry::from(instant)),
            get_delivery_price_for_expiry(&client, asset_pair, instant)
                .await
                .inspect_err(|e| warn!("Could not get delivery price for expiry: {e}"))
                .ok(),
        )),
        _ => None,
    };

    let mut as_vec_expiries = maybe_close_expiry_entry
        .into_iter()
        .chain(
            deribit_response
                .unwrap_or_default()
                .into_iter()
                .map(|(key, value)| {
                    (
                        EventId::forward_of_expiry_with_pair_at_timestamp(
                            asset_pair,
                            Expiry::try_from(key).expect("Deribit expiry is always valid"),
                            instant,
                        ),
                        Some(value),
                    )
                }),
        )
        .collect::<Vec<_>>();

    debug!("Returning these prices for event ids: {as_vec_expiries:?}",);

    as_vec_expiries.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(as_vec_expiries)
}

use serde::{de::DeserializeSeed as _, Deserialize};
use serde_json::Deserializer;

///// Helper function to deserialize the Deribit book summary response /////

use super::DeribitErrorObject;

#[derive(Debug, Clone)]
enum DeribitResponseForward<'a> {
    ResultResponse { result: HashMap<&'a str, f64> },
    ErrorResponse { error: DeribitErrorObject },
}

/// We are only interested in the underlying future of each option expiry.
/// Deribit returns a list of all their options with various strikes that we do not care about.
/// We use an optimized deserializer to extract the underlying future price for each expiry
/// that avoids over-allocating and ignores all objects once it has a response for the expected
/// number of quoting expiries.
///
/// The implementation of the deserializer is in the `custom_serde.rs` file.
mod custom_serde;
use custom_serde::DeribitResponseForwardDeserializer;

fn deserialize_deribit_response_forward(
    payload: &[u8],
    nb_expiries: usize,
) -> Option<HashMap<&str, f64>> {
    match DeribitResponseForwardDeserializer::new(nb_expiries)
        .deserialize(&mut Deserializer::from_slice(payload))
        .inspect_err(|e| {
            error!("Error deserializing Deribit response: {e:#?}");
        })
        .ok()?
    {
        DeribitResponseForward::ErrorResponse { error } => {
            warn!("{error}");
            None
        }
        DeribitResponseForward::ResultResponse { result } => {
            debug!("Got these futures prices from deribit: {result:#?}");
            Some(result)
        }
    }
}

///// Helper function to get the delivery price for an expiry /////

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
        .send()
        .await?
        .json::<DeribitDeliveryPriceResponse>()
        .await?
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
