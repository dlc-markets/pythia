use crate::data_models::asset_pair::AssetPair;
use chrono::DateTime;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};

pub(crate) mod error;
use error::Result;

#[cfg(test)]
use strum::EnumIter;

pub(crate) trait PriceFeed {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str;
    async fn retrieve_price(&self, asset_pair: AssetPair, datetime: DateTime<Utc>) -> Result<f64>;
}

mod bitstamp;
mod deribit;
mod gateio;
mod kraken;
mod lnm;

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
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
    pub async fn retrieve_price(
        &self,
        asset_pair: AssetPair,
        datetime: DateTime<Utc>,
    ) -> Result<f64> {
        match self {
            Self::Lnmarkets => lnm::Lnmarkets {}.retrieve_price(asset_pair, datetime).await,
            Self::Deribit => {
                deribit::Deribit {}
                    .retrieve_price(asset_pair, datetime)
                    .await
            }
            Self::Kraken => kraken::Kraken {}.retrieve_price(asset_pair, datetime).await,
            Self::GateIo => gateio::GateIo {}.retrieve_price(asset_pair, datetime).await,
            Self::Bitstamp => {
                bitstamp::Bitstamp {}
                    .retrieve_price(asset_pair, datetime)
                    .await
            }
        }
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
                .retrieve_price(AssetPair::BtcUsd, now)
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
