use crate::AssetPair;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use time::OffsetDateTime;

mod error;
pub use error::PriceFeedError;
pub use error::Result;

#[async_trait]
pub trait PriceFeed {
    fn translate_asset_pair(&self, asset_pair: AssetPair) -> &'static str;
    async fn retrieve_price(&self, asset_pair: AssetPair, datetime: OffsetDateTime) -> Result<f64>;
}

mod bitstamp;
mod gateio;
mod kraken;
mod lnm;

pub use bitstamp::Bitstamp;
pub use gateio::GateIo;
pub use kraken::Kraken;
pub use lnm::Lnm;

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ImplementedPriceFeed {
    Lnm,
    Kraken,
    GateIo,
    Bitstamp,
}

impl ImplementedPriceFeed {
    pub fn get_pricefeed(self) -> Box<dyn PriceFeed + Send + Sync> {
        match self {
            Self::Lnm => Box::new(lnm::Lnm {}),
            Self::Kraken => Box::new(kraken::Kraken {}),
            Self::GateIo => Box::new(gateio::GateIo {}),
            Self::Bitstamp => Box::new(bitstamp::Bitstamp {}),
        }
    }
}

// Pricefeeders can be obtain by coding a pricefeed which simply aggregate other pricefeeders response
