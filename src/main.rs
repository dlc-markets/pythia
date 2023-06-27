#[macro_use]
extern crate log;

use clap::Parser;
use hex::ToHex;
use secp256k1_zkp::{rand, KeyPair, Secp256k1, SecretKey};

use std::{collections::HashMap, fs::File, io::Read, str::FromStr};

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

// const PAGE_SIZE: u32 = 100;

#[actix_web::main]
// #[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = cli::PythiaArgs::parse();

    let mut secret_key = String::new();
    let secp = Secp256k1::new();

    let secret_key = match &args.secret_key_file {
        None => {
            info!("no secret key file was found, generating secret key");
            secp.generate_keypair(&mut rand::thread_rng()).0
        }
        Some(path) => {
            info!(
                "reading secret key from {}",
                path.as_os_str().to_string_lossy()
            );
            File::open(path)?.read_to_string(&mut secret_key)?;
            secret_key.retain(|c| !c.is_whitespace());
            SecretKey::from_str(&secret_key)?
        }
    };
    let keypair = KeyPair::from_secret_key(&secp, &secret_key);
    info!(
        "oracle keypair successfully generated, pubkey is {}",
        keypair.public_key().serialize().encode_hex::<String>()
    );

    let (asset_pair_infos, oracle_scheduler_config, port, db_connect, max_connections_postgres) =
        args.match_args()?;

    let db = DBconnection::new(db_connect, max_connections_postgres).await?;

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

    api::run_api(oracles, port).await?;

    Ok(())
}
