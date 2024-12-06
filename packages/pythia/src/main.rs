#[macro_use]
extern crate log;

use actix::spawn;
use clap::Parser;
use hex::ToHex;
use secp256k1_zkp::{All, Keypair, Secp256k1};
use std::collections::HashMap;
use std::sync::LazyLock;
use tokio::select;

mod api;
mod config;
mod error;
mod oracle;
mod pricefeeds;
mod schedule_context;

use config::cli::PythiaArgs;
use config::{AssetPair, AssetPairInfo};
use error::PythiaError;
use oracle::{postgres::DBconnection, Oracle};

static SECP: LazyLock<Secp256k1<All>> = const { LazyLock::new(Secp256k1::new) };

#[actix_web::main]
async fn main() -> Result<(), PythiaError> {
    env_logger::init();

    // Parse command line arguments and environnement variables to create CONFIG

    let args = PythiaArgs::parse();
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
    // Setup keypair and postgres DB for oracles
    let keypair = Keypair::from_secret_key(&SECP, &secret_key);
    info!(
        "oracle public key is {}",
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
            Oracle::new(asset_pair_info, db_connection.clone(), keypair)
        }))
        .collect::<HashMap<_, _>>();

    // Signal if we run in debug mode and launch the server

    if debug_mode {
        info!("!!! DEBUG MODE IS ON !!! DO NOT USE IN PRODUCTION !!!")
    };

    let (scheduler_context, api_context) =
        schedule_context::create_contexts(oracles, oracle_scheduler_config)?;

    // Spawn oracle events scheduler (announcements/attestations) and API
    // using the channel receiver for websocket.
    // In case of failure of scheduler or API, get the error and return it

    select! {
        e = spawn(schedule_context::scheduler::start_schedule(scheduler_context)) => {
            e?.map_err(PythiaError::from)
        },
        e = spawn(api::run_api_v1(api_context, port, debug_mode)) => {
            e?.map_err(PythiaError::from)
        },
    }
}
