use crate::data_models::asset_pair::AssetPair;
use crate::data_models::event_ids::EventId;
use chrono::DateTime;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};
use tokio::task::LocalSet;

pub(crate) mod error;
use error::Result;

#[cfg(test)]
use strum::EnumIter;

pub(crate) trait PriceFeed {
    /// Compute the event ids for a given asset pair and datetime of this pricefeed.
    fn compute_event_ids(&self, asset_pair: AssetPair, datetime: DateTime<Utc>) -> Vec<EventId> {
        vec![EventId::spot_from_pair_and_timestamp(asset_pair, datetime)]
    }
    /// Retrieve the prices for a given asset pair and datetime of this pricefeed.
    ///
    /// This is run in a local set so it is not required to be Send.
    /// The returned list of (event_id, price) MUST be sorted by event_id.
    /// All event_ids must be one of the event_ids computed by `compute_event_ids`
    /// for the same asset pair and datetime.
    ///
    /// If the scheduler retrieve the price for an event_id and it failed,
    /// returning None for the event_id will set a series of retry later.
    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        datetime: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>>;
}

mod bitstamp;
mod deribit;
mod gateio;
mod kraken;
mod lnm;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(EnumIter))]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImplementedPriceFeed {
    Lnmarkets,
    Deribit,
    Kraken,
    GateIo,
    Bitstamp,
}

impl ImplementedPriceFeed {
    pub fn events_at_date(&self, asset_pair: AssetPair, date: DateTime<Utc>) -> Vec<EventId> {
        match self {
            Self::Lnmarkets => lnm::Lnmarkets {}.compute_event_ids(asset_pair, date),
            Self::Deribit => deribit::Deribit {}.compute_event_ids(asset_pair, date),
            Self::Kraken => kraken::Kraken {}.compute_event_ids(asset_pair, date),
            Self::GateIo => gateio::GateIo {}.compute_event_ids(asset_pair, date),
            Self::Bitstamp => bitstamp::Bitstamp {}.compute_event_ids(asset_pair, date),
        }
    }
    pub async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        datetime: DateTime<Utc>,
    ) -> Result<Vec<(EventId, Option<f64>)>> {
        let query_local = LocalSet::new();
        let query_local_handle =
            match self {
                Self::Lnmarkets => {
                    query_local.spawn_local(lnm::Lnmarkets {}.retrieve_prices(asset_pair, datetime))
                }
                Self::Deribit => query_local
                    .spawn_local(deribit::Deribit {}.retrieve_prices(asset_pair, datetime)),
                Self::Kraken => {
                    query_local.spawn_local(kraken::Kraken {}.retrieve_prices(asset_pair, datetime))
                }
                Self::GateIo => {
                    query_local.spawn_local(gateio::GateIo {}.retrieve_prices(asset_pair, datetime))
                }
                Self::Bitstamp => query_local
                    .spawn_local(bitstamp::Bitstamp {}.retrieve_prices(asset_pair, datetime)),
            };

        query_local.await;
        let prices = query_local_handle.await.map_err(|e| {
            error::PriceFeedError::ConnectionError(format!("Error in pricefeed: {e}"))
        })??;

        assert!(
            prices.iter().is_sorted_by(|a, b| a.0 < b.0),
            "The pricefeed must always return the list of price sorted by event id"
        );

        Ok(prices)
    }
}

impl Display for ImplementedPriceFeed {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Lnmarkets => write!(f, "lnmarkets"),
            Self::Deribit => write!(f, "deribit"),
            Self::Kraken => write!(f, "kraken"),
            Self::GateIo => write!(f, "gateio"),
            Self::Bitstamp => write!(f, "bitstamp"),
        }
    }
}

// Pricefeeds can be obtain by coding a pricefeed which simply aggregate other pricefeeds response

#[cfg(test)]
mod test {
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
}
