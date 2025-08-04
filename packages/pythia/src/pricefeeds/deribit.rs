use std::collections::BTreeMap;

use super::{PriceFeed, Result};
use crate::{
    data_models::{asset_pair::AssetPair, event_ids::EventId, expiries::Expiry},
    pricefeeds::error::PriceFeedError,
};
use awc::Client;
use chrono::{DateTime, TimeDelta, Utc};
use log::debug;
use tokio::try_join;

pub(super) struct Deribit {}

mod delivery_price;
mod forward_price;
mod index_price;

#[cfg(test)]
mod test;

impl PriceFeed for Deribit {
    fn compute_event_ids(&self, asset_pair: AssetPair, datetime: DateTime<Utc>) -> Vec<EventId> {
        let spot = EventId::spot_from_pair_and_timestamp(asset_pair, datetime);
        let option_expiries = forward_price::option_expiries(datetime);

        let delivery_case = delivery_price::is_an_expiry_date(datetime).then_some(
            EventId::delivery_of_expiry_with_pair(asset_pair, Expiry::from(datetime.date_naive())),
        );

        let event_ids = Some(spot)
            .into_iter()
            .chain(delivery_case)
            .chain(option_expiries.into_iter().map(|date| {
                EventId::forward_of_expiry_with_pair_at_timestamp(
                    asset_pair,
                    Expiry::from(date.date_naive()),
                    datetime,
                )
            }))
            .collect();

        debug!("Event ids at {datetime}: {event_ids:?}");
        event_ids
    }

    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        instant: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>> {
        let client = Client::new();

        if Utc::now() - instant > TimeDelta::minutes(1) {
            // The book summary only allow to get current view of the market, so we can't attest
            // forward prices that are in the past.
            info!("Requested attesting data from the past with deribit pricefeed. Only past delivery prices can be attested");

            if delivery_price::is_an_expiry_date(instant) {
                let delivery =
                    delivery_price::retrieve_delivery_price(&client, asset_pair, instant)
                        .await
                        .ok();

                return Ok(vec![(
                    EventId::delivery_of_expiry_with_pair(
                        asset_pair,
                        Expiry::from(instant.date_naive()),
                    ),
                    delivery,
                )]);
            } else {
                return Err(PriceFeedError::PriceNotAvailable(asset_pair, instant));
            }
        };

        let (spot, mut forward_map) = try_join!(
            index_price::retrieve_index_price(&client, asset_pair, instant),
            forward_price::retrieve_dated_option_prices(&client, asset_pair, instant)
        )?;

        // We insert the spot we got to the forward map to be able to compute the term structure
        // of the forward prices for options expiry without traded future to evaluate forward prices.
        forward_map.insert(instant, spot);

        let term_structure_fun = get_term_structure_from_forward_map(&forward_map);

        let option_expiries = forward_price::option_expiries(instant);

        let delivery_case = if delivery_price::is_an_expiry_date(instant) {
            // We must communicate that we failed to get delivery to retry if this happens by returning None in case of error.
            Some((
                EventId::delivery_of_expiry_with_pair(
                    asset_pair,
                    Expiry::from(instant.date_naive()),
                ),
                delivery_price::retrieve_delivery_price(&client, asset_pair, instant)
                    .await
                    .ok(),
            ))
        } else {
            None
        };

        let all_prices = Some((
            EventId::spot_from_pair_and_timestamp(asset_pair, instant),
            Some(spot),
        ))
        .into_iter()
        .chain(delivery_case)
        .chain(option_expiries.into_iter().map(|date| {
            let price = term_structure_fun(date);
            (
                EventId::forward_of_expiry_with_pair_at_timestamp(
                    asset_pair,
                    Expiry::from(date.date_naive()),
                    instant,
                ),
                price,
            )
        }))
        .collect();

        debug!("All prices: {all_prices:?}");

        Ok(all_prices)
    }
}

#[derive(serde::Deserialize, Debug, Clone)]
pub(super) struct DeribitErrorObject {
    code: i32,
    message: String,
    data: Option<String>,
}

impl std::fmt::Display for DeribitErrorObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Deribit returned an error:\n code: {},\n message: {}{}",
            self.code,
            self.message,
            self.data
                .as_ref()
                .map(|data| format!(",\ndata: {data}"))
                .unwrap_or_default()
        )
    }
}

fn get_term_structure_from_forward_map(
    forward_map: &BTreeMap<DateTime<Utc>, f64>,
) -> impl Fn(DateTime<Utc>) -> Option<f64> + use<'_> {
    |instant: DateTime<Utc>| {
        if let Some(&price) = forward_map.get(&instant) {
            Some(price)
        } else {
            let (&next_date, &next_price) = forward_map.range(instant..).next()?;
            let (&previous_date, &previous_price) = forward_map.range(..instant).next_back()?;

            Some(
                previous_price
                    + (next_price - previous_price)
                        * (instant - previous_date).num_milliseconds() as f64
                        / (next_date - previous_date).num_milliseconds() as f64,
            )
        }
    }
}

/// Tests if we are able to approximate the same term structure as deribit correctly with their
/// non-trade able futures.
#[cfg(test)]
mod test_term_structure {

    use std::collections::HashMap;

    use super::*;

    #[actix::test]
    async fn term_structure_adequate_for_forward_prices() {
        let now = Utc::now();

        #[derive(serde::Deserialize, Debug, Clone)]
        struct OptionQuote {
            underlying_price: f64,
            underlying_index: String,
        }

        #[derive(serde::Deserialize, Debug, Clone)]
        struct DeribitOptionResponse {
            result: Vec<OptionQuote>,
        }

        let client = Client::new();

        let mut spot_fixed = false;

        let mut first_spot = 0.0;
        let mut after_spot;
        let mut result = vec![];

        while !spot_fixed {
            first_spot = index_price::retrieve_index_price(&client, AssetPair::BtcUsd, now)
                .await
                .unwrap();

            // At the same time we query deribit for future and option, hopefully we get the answer close enough for price to not deviate too much.;

            DeribitOptionResponse { result } = client
                .get(forward_price::BOOK_SUMMARY_BY_CURRENCY)
                .query(&[("kind", "option"), ("currency", "BTC")])
                .expect("can be serialized")
                .send()
                .await
                .unwrap()
                .json::<DeribitOptionResponse>()
                .await
                .unwrap();

            after_spot = index_price::retrieve_index_price(&client, AssetPair::BtcUsd, now)
                .await
                .unwrap();

            if first_spot == after_spot {
                spot_fixed = true;
            }
        }

        let spot = first_spot;

        result.sort_by(|a, b| a.underlying_index.cmp(&b.underlying_index));

        let option_quotes = result
            .chunk_by(|a, b| a.underlying_index == b.underlying_index)
            .filter_map(|quotes| {
                let expiry = if quotes[0].underlying_index.starts_with("SYN") {
                    quotes[0].underlying_index[8..].parse::<Expiry>().ok()?
                } else {
                    quotes[0].underlying_index[4..].parse::<Expiry>().ok()?
                };
                Some((
                    expiry,
                    quotes
                        .iter()
                        .map(|quote| quote.underlying_price)
                        .sum::<f64>()
                        / quotes.len() as f64,
                ))
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(
            option_quotes.len(),
            forward_price::option_expiries(now).len(),
            "Did not deserialize all option quote correctly"
        );

        let quoting_futures = forward_price::iter_all_quoting_futures_forward_at_date(now);

        let mut forward_map = quoting_futures
            .iter()
            .copied()
            .map(|date| (date, option_quotes[&Expiry::from(date.date_naive())]))
            .collect::<BTreeMap<_, _>>();

        forward_map.insert(now, spot);

        let term_structure_fun = get_term_structure_from_forward_map(&forward_map);

        let mut results_buffer = Vec::with_capacity(option_quotes.len() - quoting_futures.len());

        for (expiry, price) in option_quotes {
            if quoting_futures.contains(&expiry.into()) {
                assert_eq!(price, term_structure_fun(expiry.into()).unwrap());
                continue;
            }

            let term_structure_price = term_structure_fun(expiry.into()).unwrap();
            results_buffer.push((expiry, price, term_structure_price));
        }

        let message = results_buffer
            .iter()
            .map(|(expiry, price, term_structure_price)| {
                format!("At {expiry}, computed {term_structure_price:?}, deribit sent {price:?}")
            })
            .reduce(|a, b| format!("{a}\n{b}"))
            .unwrap();

        results_buffer
            .into_iter()
            .for_each(|(expiry, price, term_structure_price)| {
                assert!(
                    f64::abs(price - term_structure_price) < 10.0,
                    "{message}\nHigh error at {expiry}"
                );
            });
    }
}
