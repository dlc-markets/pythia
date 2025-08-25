use crate::data_models::asset_pair::AssetPair;
use crate::data_models::event_ids::EventId;
use chrono::DateTime;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};
use tokio::task::LocalSet;

pub(crate) mod error;
use error::Result;

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
#[cfg(test)]
pub mod test;

#[cfg(test)]
pub mod test_import {
    pub use std::sync::{mpsc::Receiver, Mutex};
    pub use strum::EnumIter;
}

#[cfg(test)]
use test_import::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(test, derive(EnumIter))]
#[cfg_attr(not(test), derive(PartialEq, Eq))]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImplementedPriceFeed {
    Lnmarkets,
    Deribit,
    Kraken,
    GateIo,
    Bitstamp,
    #[cfg(test)]
    ReservedForTest {
        #[serde(skip)]
        mocking_receiver: Option<&'static Mutex<Receiver<Vec<(EventId, Option<f64>)>>>>,
    },
}

#[cfg(test)]
impl PartialEq for ImplementedPriceFeed {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl ImplementedPriceFeed {
    pub fn events_at_date(&self, asset_pair: AssetPair, date: DateTime<Utc>) -> Vec<EventId> {
        match self {
            Self::Lnmarkets => lnm::Lnmarkets {}.compute_event_ids(asset_pair, date),
            Self::Deribit => deribit::Deribit {}.compute_event_ids(asset_pair, date),
            Self::Kraken => kraken::Kraken {}.compute_event_ids(asset_pair, date),
            Self::GateIo => gateio::GateIo {}.compute_event_ids(asset_pair, date),
            Self::Bitstamp => bitstamp::Bitstamp {}.compute_event_ids(asset_pair, date),
            #[cfg(test)]
            Self::ReservedForTest { mocking_receiver } => mocking_receiver
                .as_deref()
                .map(|r| {
                    r.lock().unwrap().try_recv().expect(
                        "Caller must guarantee a push into the channel sender for each call",
                    )
                })
                .unwrap_or_default()
                .into_iter()
                .map(|(e, _)| e)
                .collect(),
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
                #[cfg(test)]
                Self::ReservedForTest { mocking_receiver } => {
                    let prices = mocking_receiver
                        .as_ref()
                        .map(|r| {
                            r.lock().unwrap().try_recv().expect(
                            "Caller must guarantee a push into the channel sender for each call",
                        )
                        })
                        .unwrap_or_default();
                    query_local.spawn_local(async { Ok(prices) })
                }
            };

        query_local.await;
        let prices = query_local_handle.await.map_err(|e| {
            error::PriceFeedError::ConnectionError(format!("Error in pricefeed: {e}"))
        })??;

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
            #[cfg(test)]
            Self::ReservedForTest { .. } => write!(f, "[MOCKED]"),
        }
    }
}
