use clap::Parser;
use secp256k1_zkp::SecretKey;
use sqlx::postgres::PgConnectOptions;
use std::{
    fs::{read_to_string, File},
    io::Read,
    str::FromStr,
};

use super::{
    env::*, error::PythiaConfigError, AssetPairInfo, ConfigurationFile, OracleSchedulerConfig,
};

#[derive(Parser)]
/// Simple DLC oracle implementation
pub(crate) struct PythiaArgs {
    /// Private key, MUST be set if ORACLE_SECRET_KEY is not
    #[clap(short, long, value_name = "hex")]
    pub secret_key_file: Option<String>,

    /// Optional config file; if not provided, it is assumed to exist at "config.json"
    #[clap(short, long, value_name = "file", value_hint = clap::ValueHint::FilePath)]
    config_file: Option<std::path::PathBuf>,

    /// Optional port for API; if not provided, use 8000
    #[clap(short, long, value_name = "port")]
    port: Option<u16>,

    /// Optional postgres URL for oracle DB, if not provided use "postgres://postgres:postgres@127.0.0.1:5432/postgres"
    #[clap(short, long, value_name = "url", value_hint = clap::ValueHint::Url)]
    url_postgres: Option<PgConnectOptions>,

    /// Optional number of maximum postgres connection, default to 10 if not provided
    #[clap(short, long, value_name = "connections")]
    max_connections: Option<u32>,

    /// Debug mode: allow using /force API path DO NOT SET TO TRUE IN PRODUCTION
    #[clap(short, long, value_name = "enabled")]
    debug_mode: Option<bool>,
}

type InitParams = (
    SecretKey,
    Vec<AssetPairInfo>,
    OracleSchedulerConfig,
    u16,
    PgConnectOptions,
    u32,
    bool,
);

impl PythiaArgs {
    pub(crate) fn match_args(self) -> Result<InitParams, PythiaConfigError> {
        let config_file: ConfigurationFile = match self.config_file {
            None => {
                info!("reading asset pair and oracle scheduler config from config.json");
                serde_json::from_str(&read_to_string("config.json")?)?
            }
            Some(path) => {
                info!(
                    "reading asset pair and oracle scheduler config from {}",
                    path.as_os_str().to_string_lossy()
                );
                let mut config_file = String::new();
                File::open(path)?.read_to_string(&mut config_file)?;
                serde_json::from_str(&config_file)?
            }
        };

        let (asset_pair_infos, oracle_scheduler_config): (
            Vec<AssetPairInfo>,
            OracleSchedulerConfig,
        ) = (
            config_file.asset_pair_infos,
            config_file.oracle_scheduler_config,
        );

        info!(
            "asset pair and oracle scheduler config successfully read: {:#?}\n{}",
            &asset_pair_infos, &oracle_scheduler_config
        );

        let db_connect = self.url_postgres.unwrap_or(match_postgres_env()?);

        let secret_key = match self.secret_key_file {
            Some(s) => {
                SecretKey::from_str(s.as_str()).map_err(|_e| PythiaConfigError::InvalidSecretKey)?
            }
            None => match_secret_key_env()?,
        };

        let debug_mode = match self.debug_mode {
            Some(s) => s,
            None => match_debug_mode_env(),
        };

        let port: u16 = match self.port {
            Some(s) => s,
            None => match_port_env(),
        };

        let max_connections: u32 = match self.max_connections {
            Some(s) => s,
            None => match_nb_connection(),
        };

        Ok((
            secret_key,
            asset_pair_infos,
            oracle_scheduler_config,
            port,
            db_connect,
            max_connections,
            debug_mode,
        ))
    }
}
