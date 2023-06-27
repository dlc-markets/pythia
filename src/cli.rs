use std::{
    fs::{self, File},
    io::Read,
    str::FromStr,
};

use clap::Parser;

use crate::common::{AssetPairInfo, OracleSchedulerConfig};

use sqlx::postgres::PgConnectOptions;

use anyhow::Result;

#[derive(Parser)]
/// Simple DLC oracle implementation
pub struct PythiaArgs {
    /// Optional private key file; if not provided, one is generated
    #[clap(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    pub secret_key_file: Option<std::path::PathBuf>,

    /// Optional asset pair config file; if not provided, it is assumed to exist at "config/asset_pair.json"
    #[clap(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    asset_pair_config_file: Option<std::path::PathBuf>,

    /// Optional oracle config file; if not provided, it is assumed to exist at "config/oracle.json"
    #[clap(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    oracle_scheduler_config_file: Option<std::path::PathBuf>,

    /// Optional oracle config file; if not provided, it is assumed to exist at "config/oracle.json"
    #[clap(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    oracle_config_file: Option<std::path::PathBuf>,

    /// Optional port for API; if not provided, use 8000
    #[clap(short, long, value_name = "PORT")]
    port: Option<u16>,

    /// Optional postgres URL for oracle DB, if not provided use "postgres://postgres:postgres@127.0.0.1:5432/postgres"
    #[clap(short, long, value_name = "URL", value_hint = clap::ValueHint::Url)]
    postgres_url: Option<PgConnectOptions>,

    /// Optional number of maximum postgres connection, default to 10 if not provided
    #[clap(short, long, value_name = "NB_CONNECTIONS")]
    max_connections: Option<u32>,
}

impl PythiaArgs {
    pub fn match_args(
        self,
    ) -> Result<(
        Vec<AssetPairInfo>,
        OracleSchedulerConfig,
        u16,
        PgConnectOptions,
        u32,
    )> {
        let asset_pair_infos: Vec<AssetPairInfo> = match self.asset_pair_config_file {
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

        let oracle_scheduler_config: OracleSchedulerConfig = match self.oracle_scheduler_config_file
        {
            None => {
                info!("reading oracle config from config/oracle_scheduler.json");
                serde_json::from_str(&fs::read_to_string("config/oracle_scheduler.json")?)?
            }
            Some(path) => {
                info!(
                    "reading oracle scheduler config from {}",
                    path.as_os_str().to_string_lossy()
                );
                let mut oracle_scheduler_config = String::new();
                File::open(path)?.read_to_string(&mut oracle_scheduler_config)?;
                serde_json::from_str(&oracle_scheduler_config)?
            }
        };
        info!(
            "oracle scheduler config successfully read: {:#?}",
            oracle_scheduler_config
        );

        let port: u16 = self.port.unwrap_or(8000);

        match self.postgres_url {
            None => {
                let db_connect = PgConnectOptions::from_str(
                    "postgres://postgres:postgres@127.0.0.1:5432/postgres",
                )?;
                Ok((
                    asset_pair_infos,
                    oracle_scheduler_config,
                    port,
                    db_connect,
                    self.max_connections.unwrap_or(10),
                ))
            }
            Some(db_connect) => Ok((
                asset_pair_infos,
                oracle_scheduler_config,
                port,
                db_connect,
                self.max_connections.unwrap_or(10),
            )),
        }
    }
}
