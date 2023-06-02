#[macro_use]
extern crate log;

use actix_web::{web, App, HttpServer};
use clap::Parser;
use hex::ToHex;
use secp256k1_zkp::{rand, KeyPair, Secp256k1, SecretKey};

use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    str::FromStr,
};

mod oracle;
use oracle::Oracle;

mod common;
use common::{AssetPair, AssetPairInfo, OracleConfig};

mod error;

mod pricefeeds;
use pricefeeds::{Lnm, PriceFeed};

mod oracle_scheduler;

use crate::oracle::postgres::DBconnection;

mod api;

// const PAGE_SIZE: u32 = 100;

#[derive(Parser)]
/// Simple DLC oracle implementation
struct Args {
    /// Optional private key file; if not provided, one is generated
    #[clap(short, long, parse(from_os_str), value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    secret_key_file: Option<std::path::PathBuf>,

    /// Optional asset pair config file; if not provided, it is assumed to exist at "config/asset_pair.json"
    #[clap(short, long, parse(from_os_str), value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    asset_pair_config_file: Option<std::path::PathBuf>,

    /// Optional oracle config file; if not provided, it is assumed to exist at "config/oracle.json"
    #[clap(short, long, parse(from_os_str), value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    oracle_config_file: Option<std::path::PathBuf>,
}

#[actix_web::main]
// #[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = Args::parse();

    let mut secret_key = String::new();
    let secp = Secp256k1::new();

    let secret_key = match args.secret_key_file {
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

    let asset_pair_infos: Vec<AssetPairInfo> = match args.asset_pair_config_file {
        None => {
            info!("reading asset pair config from config/asset_pair.json");
            serde_json::from_str(&fs::read_to_string("config/asset_pair.json")?)?
        }
        Some(path) => {
            info!(
                "reading asset pair config from {}",
                path.as_os_str().to_string_lossy()
            );
            let mut asset_pair_info = String::new();
            File::open(path)?.read_to_string(&mut asset_pair_info)?;
            serde_json::from_str(&asset_pair_info)?
        }
    };
    info!(
        "asset pair config successfully read: {:#?}",
        asset_pair_infos
    );

    let oracle_config: OracleConfig = match args.oracle_config_file {
        None => {
            info!("reading oracle config from config/oracle.json");
            serde_json::from_str(&fs::read_to_string("config/oracle.json")?)?
        }
        Some(path) => {
            info!(
                "reading oracle config from {}",
                path.as_os_str().to_string_lossy()
            );
            let mut oracle_config = String::new();
            File::open(path)?.read_to_string(&mut oracle_config)?;
            serde_json::from_str(&oracle_config)?
        }
    };
    info!("oracle config successfully read: {:#?}", oracle_config);

    const DB_URL: &str = "postgres://postgres:postgres@127.0.0.1:5432/postgres";
    let db = DBconnection::new(DB_URL, 10).await?;

    // setup event databases
    let oracles = asset_pair_infos
        .iter()
        .map(|asset_pair_info| asset_pair_info.asset_pair)
        .zip(asset_pair_infos.iter().cloned().map(|asset_pair_info| {
            let asset_pair = asset_pair_info.asset_pair;

            // pricefeed retreival
            info!("creating pricefeeds for {}", asset_pair);
            let pricefeed: Box<dyn PriceFeed + Send + Sync> = Box::new(Lnm {});

            info!("creating oracle for {}", asset_pair);
            let oracle = Oracle::new(
                oracle_config,
                asset_pair_info,
                secp.clone(),
                db.clone(),
                pricefeed,
                keypair,
            )?;

            info!("scheduling oracle events for {}", asset_pair);
            // schedule oracle events (announcements/attestations)
            oracle_scheduler::init(oracle.clone().into())?;

            Ok(oracle)
        }))
        .map(|(asset_pair, oracle)| oracle.map(|ok| (asset_pair, ok)))
        .collect::<anyhow::Result<HashMap<_, _>>>()?;

    // setup and run server
    info!("starting server");
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(oracles.clone()))
            .service(
                web::scope("/v1")
                    // .service(announcements)
                    .service(api::announcement)
                    .service(api::config)
                    .service(api::pubkey),
            )
    })
    .bind(("127.0.0.1", 8000))?
    .run()
    .await?;

    Ok(())
}
