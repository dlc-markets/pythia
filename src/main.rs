#[macro_use]
extern crate log;

use clap::Parser;
use error::PythiaError;
use hex::ToHex;
use secp256k1_zkp::{All, KeyPair, Secp256k1};
use tokio::{select, sync::broadcast};

use futures::future::TryFutureExt;

use std::{collections::HashMap, sync::OnceLock};

mod oracle;
use oracle::Oracle;

pub(crate) mod config;
use config::{AssetPair, AssetPairInfo};

mod pricefeeds;
mod scheduler;

use crate::{
    api::EventNotification,
    config::{cli, OracleSchedulerConfig},
    oracle::postgres::DBconnection,
};

pub(crate) mod api;

mod error;

// Configuration and context maintained during Pythia execution

static CONFIG: OnceLock<(Box<[AssetPairInfo]>, OracleSchedulerConfig)> = OnceLock::new();
static SECP: OnceLock<Secp256k1<All>> = OnceLock::new();
static KEYPAIR: OnceLock<KeyPair> = OnceLock::new();
static DB: OnceLock<DBconnection> = OnceLock::new();
static ORACLES: OnceLock<HashMap<AssetPair, Oracle<'static>>> = OnceLock::new();

#[actix_web::main]
async fn main() -> Result<(), PythiaError> {
    env_logger::init();

    // Parse command line arguments and environnement variables to create CONFIG

    let args = cli::PythiaArgs::parse();
    let (
        secret_key,
        asset_pair_infos,
        oracle_scheduler_config,
        port,
        db_connect,
        max_connections_postgres,
        debug_mode,
    ) = args.match_args()?;
    CONFIG.get_or_init(|| (asset_pair_infos.into_boxed_slice(), oracle_scheduler_config));
    // Setup secp context, keypair and postgres DB for oracles

    SECP.get_or_init(|| Secp256k1::new());
    KEYPAIR.get_or_init(|| KeyPair::from_secret_key(SECP.get().unwrap(), &secret_key));
    info!(
        "oracle pubkey is {}",
        KEYPAIR
            .get()
            .unwrap()
            .public_key()
            .serialize()
            .encode_hex::<String>()
    );
    let db_connection = DBconnection::new(db_connect, max_connections_postgres).await?;
    DB.get_or_init(|| db_connection);
    DB.get().unwrap().migrate().await?;

    // Setup one oracle for each asset pair found in configuration file

    ORACLES.get_or_init(|| {
        CONFIG
            .get()
            .unwrap()
            .0
            .iter()
            .map(|asset_pair_info| asset_pair_info.asset_pair)
            .zip(CONFIG.get().unwrap().0.iter().map(|asset_pair_info| {
                let asset_pair = asset_pair_info.asset_pair;

                info!("creating oracle for {}", asset_pair);
                let oracle: Oracle<'static> = Oracle::new(
                    asset_pair_info,
                    SECP.get().unwrap(),
                    DB.get().unwrap(),
                    KEYPAIR.get().unwrap(),
                );

                info!("scheduling oracle events for {}", asset_pair);

                oracle
            }))
            .collect::<HashMap<_, _>>()
    });

    // Signal if we run in debug mode and launch the server

    if debug_mode {
        info!("!!! DEBUG MODE IS ON !!! DO NOT USE IN PRODUCTION !!!")
    };

    // Initialise a channel to send from scheduler to websocket new announcements/attestations

    // We set channel size to 2 because it may happen that an announcement and attestation are sent into the channel at the same time
    // (if offset is a multiple of the attestation frequency schedule)

    let (attestation_tx, attestation_rx) = broadcast::channel::<EventNotification>(2);

    // schedule oracle events (announcements/attestations) and start API using the channel receiver for websocket
    // In case of failure of scheduler or API, get the error and return it

    select!(
        e = scheduler::start_schedule(
            ORACLES.get().unwrap(),
            &CONFIG.get().unwrap().1,
            attestation_tx,
        ) => {e},
        e = api::run_api(
            (
                ORACLES.get().unwrap(),
                &CONFIG.get().unwrap().1,
                attestation_rx.into(),
                debug_mode,
            ),
            port,
        ).err_into() => {e}
    )
}
