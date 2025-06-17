use crate::data_models::asset_pair::AssetPair;
use crate::data_models::event_ids::EventId;
use chrono::DateTime;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};

pub(crate) mod error;
use error::Result;

#[cfg(test)]
use strum::EnumIter;

pub(crate) trait PriceFeed {
    fn compute_event_ids(&self, asset_pair: AssetPair, datetime: DateTime<Utc>) -> Vec<EventId> {
        vec![EventId::spot_from_pair_and_timestamp(asset_pair, datetime)]
    }
    async fn retrieve_prices(
        &self,
        asset_pair: AssetPair,
        datetime: DateTime<Utc>,
    ) -> Result<Vec<(EventId, f64)>>;
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
    ) -> Result<Vec<(EventId, f64)>> {
        let prices = match self {
            Self::Lnmarkets => {
                lnm::Lnmarkets {}
                    .retrieve_prices(asset_pair, datetime)
                    .await
            }
            Self::Deribit => {
                deribit::Deribit {}
                    .retrieve_prices(asset_pair, datetime)
                    .await
            }
            Self::Kraken => {
                kraken::Kraken {}
                    .retrieve_prices(asset_pair, datetime)
                    .await
            }
            Self::GateIo => {
                gateio::GateIo {}
                    .retrieve_prices(asset_pair, datetime)
                    .await
            }
            Self::Bitstamp => {
                bitstamp::Bitstamp {}
                    .retrieve_prices(asset_pair, datetime)
                    .await
            }
        }?;

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
            panic!(
                "Some pricefeed APIs seem deprecated: {:?}\n No answer for date {}",
                deprecated, now
            )
        }
    }
}
