use crate::AssetPair;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::{Deserialize, Serialize};

pub(crate) mod error;
use error::Result;

#[cfg(test)]
use strum::EnumIter;

#[async_trait]
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
    pub fn get_pricefeed(self) -> Box<dyn PriceFeed + Send + Sync> {
        match self {
            Self::Lnmarkets => Box::new(lnm::Lnmarkets {}),
            Self::Deribit => Box::new(deribit::Deribit {}),
            Self::Kraken => Box::new(kraken::Kraken {}),
            Self::GateIo => Box::new(gateio::GateIo {}),
            Self::Bitstamp => Box::new(bitstamp::Bitstamp {}),
        }
    }
}

// Pricefeeders can be obtain by coding a pricefeed which simply aggregate other pricefeeders response

#[cfg(test)]
mod test {
    use chrono::{SubsecRound, Utc};

    use crate::config::AssetPair;

    use super::{error::PriceFeedError, ImplementedPriceFeed};
    use strum::IntoEnumIterator;

    // Test all the implemented pricefeeders. Failing mean there has been breaking change in a pricefeeder API
    #[actix_web::test]
    async fn test_all_pricefeeders() {
        let mut deprecated: Vec<(ImplementedPriceFeed, PriceFeedError)> = vec![];
        let now = Utc::now().trunc_subsecs(0);
        for pricefeed in ImplementedPriceFeed::iter() {
            let _ = pricefeed
                .get_pricefeed()
                .retrieve_price(AssetPair::BtcUsd, now)
                .await
                .map_err(|e| deprecated.push((pricefeed, e)));
        }
        if !deprecated.is_empty() {
            panic!(
                "Some pricefeeder APIs seem deprecated: {:?}\n No answer for date {}",
                deprecated, now
            )
        }
    }
}
