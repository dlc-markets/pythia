#[macro_use]
extern crate log;

use clap::Parser;
use error::PythiaError;
use hex::ToHex;
use secp256k1_zkp::{KeyPair, Secp256k1};
use tokio::{select, sync::broadcast};

use futures::future::TryFutureExt;

use std::{collections::HashMap, sync::Arc};

mod oracle;
use oracle::Oracle;

pub(crate) mod config;
use config::{AssetPair, AssetPairInfo};

mod pricefeeds;
mod scheduler;

use crate::{api::EventNotification, config::cli, oracle::postgres::DBconnection};

pub(crate) mod api;

mod error;

// const PAGE_SIZE: u32 = 100;

#[actix_web::main]
// #[tokio::main]
async fn main() -> Result<(), PythiaError> {
    env_logger::init();

    // Parse command line arguments and environnement variables

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

    // Setup keypair and postgres DB for oracles

    let secp = Secp256k1::new();
    let keypair = KeyPair::from_secret_key(&secp, &secret_key);
    info!(
        "oracle pubkey is {}",
        keypair.public_key().serialize().encode_hex::<String>()
    );

    let db = DBconnection::new(db_connect, max_connections_postgres).await?;

    db.migrate().await?;

    // Setup one oracle for each asset pair found in configuration file
    let oracles = asset_pair_infos
        .iter()
        .map(|asset_pair_info| asset_pair_info.asset_pair)
        .zip(asset_pair_infos.iter().cloned().map(|asset_pair_info| {
            let asset_pair = asset_pair_info.asset_pair;

            info!("creating oracle for {}", asset_pair);
            let oracle = Oracle::new(asset_pair_info, secp.clone(), db.clone(), keypair);

            info!("scheduling oracle events for {}", asset_pair);

            oracle
        }))
        .map(|(asset_pair, oracle)| (asset_pair, Arc::new(oracle)))
        .collect::<HashMap<_, _>>();

    // Signal if we run in debug mode and launch the server

    if debug_mode {
        info!("!!! DEBUG MODE IS ON !!! DO NOT USE IN PRODUCTION !!!")
    };

    // Initialise channel to send from scheduler to websocket new announcements/attestations
    let (attestation_tx, attestation_rx) = broadcast::channel::<EventNotification>(1);

    // schedule oracle events (announcements/attestations) and start API using the channel receiver for websocket
    // In case of failure of scheduler or API, get the error and return it
    select! {
        e = scheduler::start_schedule(
            oracles.clone().into_values().collect(),
            &oracle_scheduler_config,
            attestation_tx,
        ) => {e},
        e = api::run_api(
            (
                oracles,
                oracle_scheduler_config.clone(),
                attestation_rx.into(),
                debug_mode,
            ),
            port,
        )
        .err_into() => {e}
    }
}
