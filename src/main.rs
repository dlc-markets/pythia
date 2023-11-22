#[macro_use]
extern crate log;

use clap::Parser;
use hex::ToHex;
use secp256k1_zkp::{KeyPair, Secp256k1};

use std::collections::HashMap;

mod oracle;
use oracle::Oracle;

mod common;
use common::{AssetPair, AssetPairInfo};

mod error;

mod pricefeeds;

mod oracle_scheduler;

use crate::oracle::postgres::DBconnection;

mod api;
mod cli;
mod env;

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

    // setup event databases
    let oracles = asset_pair_infos
        .iter()
        .map(|asset_pair_info| asset_pair_info.asset_pair)
        .zip(asset_pair_infos.iter().cloned().map(|asset_pair_info| {
            let asset_pair = asset_pair_info.asset_pair;

            info!("creating oracle for {}", asset_pair);
            let oracle = Oracle::new(asset_pair_info, secp.clone(), db.clone(), keypair)?;

            info!("scheduling oracle events for {}", asset_pair);
            // schedule oracle events (announcements/attestations)
            oracle_scheduler::init(oracle.clone().into(), oracle_scheduler_config)?;

            Ok(oracle)
        }))
        .map(|(asset_pair, oracle)| oracle.map(|ok| (asset_pair, ok)))
        .collect::<anyhow::Result<HashMap<_, _>>>()?;

    // setup and run server
    if debug_mode {
        info!(
            "!!! DEBUG MODE IS ON !!! DO NOT USE IN PRODUCTION !!! DATA INTEGRITY IS NOT GARANTED !!!"
        )
    };

    api::run_api((oracles, oracle_scheduler_config, debug_mode), port).await?;

    Ok(())
}
