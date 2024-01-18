#[macro_use]
extern crate log;

use clap::Parser;
use hex::ToHex;
use secp256k1_zkp::{KeyPair, Secp256k1};
use tokio::sync::broadcast;

use std::{collections::HashMap, sync::Arc};

mod oracle;
use oracle::Oracle;

pub(crate) mod config;
use config::{AssetPair, AssetPairInfo};

mod error;

mod pricefeeds;

mod scheduler;

use crate::{api::ws::EventNotification, oracle::postgres::DBconnection};

pub mod api;
use config::cli;
use config::env;

// const PAGE_SIZE: u32 = 100;

#[actix_web::main]
// #[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = cli::PythiaArgs::parse();

    let secp = Secp256k1::new();

    let (
        secret_key,
        asset_pair_infos,
        oracle_scheduler_config,
        port,
        db_connect,
        max_connections_postgres,
        debug_mode,
    ) = args.match_args()?;
    let keypair = KeyPair::from_secret_key(&secp, &secret_key);
    info!(
        "oracle pubkey is {}",
        keypair.public_key().serialize().encode_hex::<String>()
    );

    let db = DBconnection::new(db_connect, max_connections_postgres).await?;

    db.migrate().await?;

    // Initialise websocket event channel
    let (attestation_tx, attestation_rx) = broadcast::channel::<EventNotification>(1);

    // setup event databases
    let oracles = asset_pair_infos
        .iter()
        .map(|asset_pair_info| asset_pair_info.asset_pair)
        .zip(asset_pair_infos.iter().cloned().map(|asset_pair_info| {
            let asset_pair = asset_pair_info.asset_pair;

            info!("creating oracle for {}", asset_pair);
            let oracle = Oracle::new(asset_pair_info, secp.clone(), db.clone(), keypair)?;

            info!("scheduling oracle events for {}", asset_pair);

            Ok(oracle)
        }))
        .map(|(asset_pair, oracle)| oracle.map(|ok| (asset_pair, Arc::new(ok))))
        .collect::<anyhow::Result<HashMap<_, _>>>()?;

    // setup and run server
    if debug_mode {
        info!("!!! DEBUG MODE IS ON !!! DO NOT USE IN PRODUCTION !!!")
    };

    // schedule oracle events (announcements/attestations) and start API
    // In case of failure of scheduler or API, get the error and return it

    tokio::try_join!(
        scheduler::start_schedule(
            oracles.clone().into_values().collect(),
            &oracle_scheduler_config,
            attestation_tx.clone(),
        ),
        api::run_api(
            (
                oracles,
                oracle_scheduler_config.clone(),
                attestation_rx.into(),
                debug_mode,
            ),
            port,
        )
    )?;
    Ok(())
}
