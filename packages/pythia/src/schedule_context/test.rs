use std::{
    collections::HashMap,
    str::FromStr,
    sync::{
        Mutex,
        mpsc::{Sender, channel},
    },
};

use cron::Schedule;
use secp256k1_zkp::{Keypair, Secp256k1, rand};
use sqlx::PgPool;

use crate::{
    AssetPairInfo,
    data_models::{
        asset_pair::AssetPair, event_ids::EventIdInfos, oracle_msgs::DigitDecompositionEventDesc,
    },
    db::DBconnection,
    oracle::Oracle,
    pricefeeds::ImplementedPriceFeed,
};

use super::*;

/// A mock implementation of OracleContext for testing
#[derive(Clone)]
pub struct MockContext {
    db: DBconnection,
    oracles: HashMap<AssetPair, Oracle>,
    schedule: Schedule,
}

impl OracleContext for MockContext {
    fn db(&self) -> &DBconnection {
        &self.db
    }
    fn oracles(&self) -> &HashMap<AssetPair, Oracle> {
        &self.oracles
    }

    fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn send_error(&self, _error: OracleError) {}
}

pub type HandlerToMock = Sender<Vec<(EventIdInfos, Option<f64>)>>;

impl MockContext {
    pub async fn new(pool: PgPool) -> (HandlerToMock, Self) {
        // Create a default hourly schedule for tests
        let schedule = Schedule::from_str("0 */1 * * * * *").expect("Valid cron schedule");

        // Create mock asset pair infos for testing
        let mut oracles = HashMap::new();

        let (sender, mocking_receiver) = channel::<Vec<(EventIdInfos, Option<f64>)>>();

        // We'll simulate having a BTC/USD oracle using the mocked pricefeed
        let btc_usd_asset_pair_info = AssetPairInfo {
            pricefeed: ImplementedPriceFeed::ReservedForTest {
                mocking_receiver: Some(Box::leak(Box::new(Mutex::new(mocking_receiver)))),
            },
            asset_pair: AssetPair::BtcUsd,
            event_descriptor: DigitDecompositionEventDesc {
                base: 2,
                is_signed: false,
                unit: "usd/btc".parse().expect("usd/btc len is 7"),
                precision: 7,
                nb_digits: 30,
            },
        };

        let secp = Secp256k1::new();
        let (secret_key, _) = secp.generate_keypair(&mut rand::thread_rng());
        let keypair = Keypair::from_secret_key(&secp, &secret_key);

        // Wrap a mock DB connection for testing
        let db_connection = DBconnection(pool);

        // Create an Oracle with our mock DB connection
        let btc_usd_oracle = Oracle::new(btc_usd_asset_pair_info, keypair);

        // Add the oracle to our map (same as in main.rs)
        oracles.insert(AssetPair::BtcUsd, btc_usd_oracle);

        // Return mocked context
        (
            sender,
            Self {
                db: db_connection,
                oracles,
                schedule,
            },
        )
    }
}
