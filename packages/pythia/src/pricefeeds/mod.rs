use crate::data_models::asset_pair::AssetPair;
use crate::data_models::event_ids::EventIdInfos;
use awc::Client;
use chrono::DateTime;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::{
    cell::LazyCell,
    fmt::{self, Display, Formatter},
};

pub(crate) mod error;
use error::Result;

thread_local! {
    static HTTP_CLIENT: LazyCell<Client> = LazyCell::new(Client::new);
}

pub(crate) trait PriceFeed {
    /// Compute the event ids for a given asset pair and datetime of this pricefeed.
    fn to_schedule_events(
        &self,
        asset_pair: AssetPair,
        datetime: DateTime<Utc>,
    ) -> Vec<EventIdInfos> {
        vec![EventIdInfos::spot_from_pair_and_timestamp(
            asset_pair, datetime,
        )]
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
        event_ids_infos: impl IntoIterator<Item = EventIdInfos>,
    ) -> Result<Vec<(EventIdInfos, Option<f64>)>>;
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
    pub use std::sync::{Mutex, mpsc::Receiver};
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
        mocking_receiver: Option<&'static Mutex<Receiver<Vec<(EventIdInfos, Option<f64>)>>>>,
    },
}

#[cfg(test)]
impl PartialEq for ImplementedPriceFeed {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl ImplementedPriceFeed {
    pub const fn average_events_per_maturity(&self) -> NonZeroUsize {
        const ONE: NonZeroUsize = NonZeroUsize::new(1).unwrap();
        match self {
            Self::Lnmarkets => ONE,
            Self::Deribit => ONE,
            Self::Kraken => ONE,
            Self::GateIo => ONE,
            Self::Bitstamp => ONE,
            #[cfg(test)]
            Self::ReservedForTest { .. } => ONE,
        }
    }

    pub fn to_schedule_events(
        &self,
        asset_pair: AssetPair,
        date: DateTime<Utc>,
    ) -> Vec<EventIdInfos> {
        match self {
            Self::Lnmarkets => lnm::Lnmarkets {}.to_schedule_events(asset_pair, date),
            Self::Deribit => deribit::Deribit {}.to_schedule_events(asset_pair, date),
            Self::Kraken => kraken::Kraken {}.to_schedule_events(asset_pair, date),
            Self::GateIo => gateio::GateIo {}.to_schedule_events(asset_pair, date),
            Self::Bitstamp => bitstamp::Bitstamp {}.to_schedule_events(asset_pair, date),
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
        event_ids_infos: Vec<EventIdInfos>,
    ) -> Result<Vec<(EventIdInfos, Option<f64>)>> {
        let events_ids = event_ids_infos
            .iter()
            .map(|e| e.as_event_id())
            .collect::<Vec<_>>();
        let query_local_handle = match self {
            Self::Lnmarkets => actix::spawn(lnm::Lnmarkets {}.retrieve_prices(event_ids_infos)),
            Self::Deribit => actix::spawn(deribit::Deribit {}.retrieve_prices(event_ids_infos)),
            Self::Kraken => actix::spawn(kraken::Kraken {}.retrieve_prices(event_ids_infos)),
            Self::GateIo => actix::spawn(gateio::GateIo {}.retrieve_prices(event_ids_infos)),
            Self::Bitstamp => actix::spawn(bitstamp::Bitstamp {}.retrieve_prices(event_ids_infos)),
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
                actix::spawn(async { Ok(prices) })
            }
        };

        let prices = query_local_handle.await.map_err(|e| {
            error::PriceFeedError::ConnectionError(format!("Error in pricefeed: {e}"))
        })??;

        assert!(
            prices
                .iter()
                .enumerate()
                .all(|(i, (e, _))| events_ids[i] == e.as_event_id()),
            "All event ids must be returned by the pricefeed in the same order"
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
            #[cfg(test)]
            Self::ReservedForTest { .. } => write!(f, "[MOCKED]"),
        }
    }
}

// Pricefeeds can be obtain by coding a pricefeed which simply aggregate other pricefeeds response
