#[macro_use]
extern crate log;

use actix_web::web;
use clap::Parser;
use futures::future::TryFutureExt;
use hex::ToHex;
use secp256k1_zkp::{KeyPair, Secp256k1};
use std::collections::HashMap;
use tokio::{select, sync::broadcast};

pub(crate) mod api;
pub(crate) mod config;

mod error;
mod oracle;
mod pricefeeds;
mod scheduler;

use api::EventNotification;
use config::cli;
use config::{AssetPair, AssetPairInfo};
use error::PythiaError;
use oracle::{postgres::DBconnection, Oracle};

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
    let config = asset_pair_infos.into_boxed_slice();
    // Setup secp context, keypair and postgres DB for oracles

    let secp = Secp256k1::new();
    let keypair = KeyPair::from_secret_key(&secp, &secret_key);
    info!(
        "oracle pubkey is {}",
        keypair.public_key().serialize().encode_hex::<String>()
    );
    let db_connection = DBconnection::new(db_connect, max_connections_postgres).await?;
    db_connection.migrate().await?;

    // Setup one oracle for each asset pair found in configuration file

    let oracles: HashMap<AssetPair, Oracle> = config
        .iter()
        .map(|asset_pair_info| asset_pair_info.asset_pair)
        .zip(config.iter().cloned().map(|asset_pair_info| {
            let asset_pair = asset_pair_info.asset_pair;

            info!("creating oracle for {}", asset_pair);
            let oracle = Oracle::new(
                asset_pair_info,
                secp.clone(),
                db_connection.clone(),
                keypair,
            );

            oracle
        }))
        .collect::<HashMap<_, _>>();

    // Signal if we run in debug mode and launch the server

    if debug_mode {
        info!("!!! DEBUG MODE IS ON !!! DO NOT USE IN PRODUCTION !!!")
    };

    // Initialise a channel to send from scheduler to websocket new announcements/attestations

    // We set channel size to 2 for each oracle because it may happen that an announcement and attestation are sent into the channel
    // at the same time (if offset is a multiple of the attestation frequency schedule)

    let (attestation_tx, attestation_rx) =
        broadcast::channel::<EventNotification>(2 * config.len());
    let context = web::Data::new((oracles, oracle_scheduler_config, attestation_rx.into()));

    // schedule oracle events (announcements/attestations) and start API using the channel receiver for websocket
    // In case of failure of scheduler or API, get the error and return it

    select!(
        e = scheduler::start_schedule(
            &context.0,
            &context.1,
            attestation_tx,
        ) => {e},
        e = api::run_api(
            context.clone(),
            port,
            debug_mode,
        ).err_into() => {e}
    )
}
