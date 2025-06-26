use std::sync::LazyLock;

use super::Result;
use crate::{
    data_models::{asset_pair::AssetPair, event_ids::EventId, expiries::Expiry},
    pricefeeds::error::PriceFeedError,
};
use chrono::{DateTime, TimeDelta, Timelike, Utc};
use cron::Schedule;
use log::debug;
use reqwest::Client;

mod custom_serde;

use serde::{de::DeserializeSeed as _, Deserialize};
use serde_json::Deserializer;

#[cfg(test)]
mod test;

// A frequency class of deribit expiry along with the introduction to quotation schedule
// and the number of expiries to expect in this frequency class.
struct ForwardConfig {
    expiry_schedule: LazyLock<Schedule>,
    quotation_schedule: LazyLock<Schedule>,
    nb_expiries: usize,
}

// Deribit quotes 4 type of expiries: 3 daily, 3 weekly, 3 monthly and 4 quarterly
static DERIBIT_QUOTING_FORWARD_CONFIG: [ForwardConfig; 4] = [
    ForwardConfig {
        expiry_schedule: LazyLock::new(|| "0 0 8 * * * *".parse().expect("our valid schedule")),
        quotation_schedule: LazyLock::new(|| "0 0 8 * * * *".parse().expect("our valid schedule")),
        nb_expiries: 3,
    },
    ForwardConfig {
        expiry_schedule: LazyLock::new(|| {
            "0 0 8 * * FRIDAY *".parse().expect("our valid schedule")
        }),
        quotation_schedule: LazyLock::new(|| {
            "0 0 8 * * THURSDAY *".parse().expect("our valid schedule")
        }),
        nb_expiries: 3,
    },
    ForwardConfig {
        expiry_schedule: LazyLock::new(|| {
            "0 0 8 * * FRIDAYL *".parse().expect("our valid schedule")
        }),
        quotation_schedule: LazyLock::new(|| {
            "0 0 8 * * THURSDAYL *".parse().expect("our valid schedule")
        }),
        nb_expiries: 3,
    },
    ForwardConfig {
        expiry_schedule: LazyLock::new(|| {
            "0 0 8 * 3,6,9,12 FRIDAYL *"
                .parse()
                .expect("our valid schedule")
        }),
        quotation_schedule: LazyLock::new(|| {
            "0 0 8 * 3,6,9,12 THURSDAYL *"
                .parse()
                .expect("our valid schedule")
        }),
        nb_expiries: 4,
    },
];

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

    let delivery_case = if is_an_expiry_date(instant) {
        let delivery_price = get_delivery_price_for_expiry(&client, asset_pair, instant)
            .await
            .inspect_err(|e| warn!("Could not get delivery price for expiry: {}", e))
            .ok();
        Some((
            EventId::delivery_of_expiry_with_pair(asset_pair, Expiry::from(instant)),
            delivery_price,
        ))
    } else {
        None
    };

    let mut expiries = get_all_quoting_forward_at_date(instant).collect::<Vec<_>>();
    expiries.sort_unstable();
    expiries.dedup();
    let nb_expiries = expiries.len();
    let closest_expiry = expiries[0];

    let response_payload = if Utc::now() - instant > TimeDelta::minutes(1) {
        info!("Requested attesting data from the past with deribit pricefeed. Only delivery prices can be attested");
        None
    } else {
        let currency = match asset_pair {
            AssetPair::BtcUsd => "BTC",
        };

        if let Ok(result) = client
            .get("https://www.deribit.com/api/v2/public/get_book_summary_by_currency")
            .query(&[("kind", "option"), ("currency", currency)])
            .send()
            .await
        {
            result.bytes().await.ok()
        } else {
            None
        }
    };

    let mut deribit_response = response_payload.as_ref().and_then(|payload| {
        // We are only interested in underlying future of each option expiry.
        // Deribit returns a list of all their options with various strike that we do not care about.
        // We use an optimized deserializer to extract the underlying future price for each expiry
        // that avoid over allocating and ignore all objects once it has a response for the expected
        // number of quoting expiries.
        let serialized = custom_serde::DeribitResponseForwardDeserializer::new(nb_expiries)
            .deserialize(&mut Deserializer::from_slice(payload))
            .inspect_err(|e| {
                error!("Error deserializing Deribit response: {:#?}", e);
            })
            .ok()?;
        debug!("Got these futures prices from deribit: {:#?}", serialized);
        Some(serialized.result)
    });

    let soon_expiry_case = match (
        deribit_response.as_mut().and_then(|r| r.remove("RY")),
        delivery_case,
    ) {
        (price @ Some(_), None) => Some((
            EventId::forward_of_expiry_with_pair_at_timestamp(
                asset_pair,
                Expiry::from(closest_expiry),
                instant,
            ),
            price,
        )),
        (_, Some(delivery_price)) => Some(delivery_price),
        (None, None) => None,
    };

    let mut as_vec_expiries = soon_expiry_case
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

    debug!(
        "Returning these prices for event ids: {:?}",
        as_vec_expiries
    );

    as_vec_expiries.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(as_vec_expiries)
}

fn get_all_quoting_forward_at_date(date: DateTime<Utc>) -> impl Iterator<Item = DateTime<Utc>> {
    DERIBIT_QUOTING_FORWARD_CONFIG.iter().flat_map(
        move |&ForwardConfig {
                  ref expiry_schedule,
                  ref quotation_schedule,
                  nb_expiries,
              }| {
            let mut expiries_iter = expiry_schedule.after(&date).peekable();

            let next_expiry = expiries_iter
                .peek()
                .copied()
                .expect("Our schedules always have a next date");

            let next_quotation_introduction = quotation_schedule
                .after(&date)
                .next()
                .expect("Our schedules always have a next date");

            // If the next quotation introduction is after the next expiry, then a new expiry
            // was introduced at date without another being expired yet.
            // So we must consider one more expiry than specified by nb_expiries.
            let quoting_expiries = if next_expiry < next_quotation_introduction {
                nb_expiries + 1
            } else {
                nb_expiries
            };

            expiries_iter.take(quoting_expiries)
        },
    )
}

fn is_an_expiry_date(date: DateTime<Utc>) -> bool {
    // All expiries are every day at 8:00:00 UTC.
    date.hour() == 8 && date.minute() == 0 && date.second() == 0
}

fn forward_event_id(asset_pair: AssetPair, expiry: &str, instant: DateTime<Utc>) -> Box<str> {
    let zero_if_low_day = (expiry.len() != 7).then_some("0").unwrap_or_default();
    (asset_pair.to_string()
        + "_"
        + zero_if_low_day
        + expiry
        + "f"
        + &instant.timestamp().to_string())
        .into_boxed_str()
}

#[derive(Debug, Deserialize)]
struct DeribitDeliveryPriceResponse {
    result: DeribitDeliveryPriceData,
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

/// Query the Deribit API to get the delivery price for the given instant and index.
async fn get_delivery_price_for_expiry(
    client: &Client,
    asset_pair: AssetPair,
    instant: DateTime<Utc>,
) -> Result<f64> {
    let asset_pair_translation = match asset_pair {
        AssetPair::BtcUsd => "btc_usd",
    };

    let days = (Utc::now() - instant).num_days().to_string();

    // Make the API request
    let response = client
        .get("https://www.deribit.com/api/v2/public/get_delivery_prices")
        .query(&[
            ("index_name", asset_pair_translation),
            ("count", "1"),
            ("offset", &days),
        ])
        .send()
        .await?
        .json::<DeribitDeliveryPriceResponse>()
        .await?;

    debug!(
        "Got this delivery price response from deribit: {:#?}",
        response
    );

    // Search for matching price in results
    response
        .result
        .data
        .first()
        .and_then(|price| {
            (price.date == instant.format("%Y-%m-%d").to_string()).then_some(price.delivery_price)
        })
        .ok_or(PriceFeedError::PriceNotAvailable(asset_pair, instant))
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct DeribitQuoteForward {
    underlying_price: f64,
    creation_timestamp: u64,
}
