use chrono::Duration;
use clap::Parser;
use cron::Schedule;
use secp256k1_zkp::SecretKey;
use sqlx::postgres::PgConnectOptions;
use std::{fs::File, io::Read, str::FromStr};

use super::{
    env::*, error::PythiaConfigError, AssetPairInfo, ConfigurationFile, OracleSchedulerConfig,
};

#[derive(Parser)]
/// Simple numerical DLC oracle implementation
pub(crate) struct PythiaArgs {
    /// Private key, MUST be set if ORACLE_SECRET_KEY is not set in environment
    #[clap(short, long, value_name = "32 bits hex")]
    pub secret_key: Option<String>,

    /// Attestation schedule in Cron notation, requires offset argument, use the config file if not provided
    #[clap(long, requires("offset"), value_name = "cron schedule")]
    schedule: Option<Schedule>,

    /// Announcement publication offset before attestation, requires schedule argument, use the config file if not provided
    #[clap(
        long,
        requires("schedule"),
        value_name = "duration",
        value_parser(parse_duration)
    )]
    offset: Option<Duration>,

    /// Event Description of a supported price pair in JSON string format, multiple pairs can be configured, use the config file if none
    #[clap(
        long,
        value_name = "AssetPairInfosJSON",
        value_parser(parse_asset_pair_array)
    )]
    pair: Option<Vec<AssetPairInfo>>,

    /// Optional config file; if none provided but is required for missing oracle configuration, it is assumed to exist at "config.json"
    #[clap(short, long, value_name = "file", value_hint = clap::ValueHint::FilePath)]
    config_file: Option<std::path::PathBuf>,

    /// Optional port for the API; if not provided, use PYTHIA_PORT environment value or default to 8000
    #[clap(short, long, value_name = "port")]
    port: Option<u16>,

    /// Optional postgres URL for DB, use environment variable if none or default to "postgres://postgres:postgres@127.0.0.1:5432/postgres"
    #[clap(short, long, value_name = "url", value_hint = clap::ValueHint::Url)]
    url_postgres: Option<PgConnectOptions>,

    /// Optional number of maximum postgres connection, default to 10 if not provided
    #[clap(short, long, value_name = "connections")]
    max_connections: Option<u32>,

    /// Debug mode: allows using /force API path DO NOT SET TO TRUE IN PRODUCTION
    #[clap(short, long)]
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
        let cli_schedule =
            self.schedule
                .zip(self.offset)
                .map(|(schedule, offset)| OracleSchedulerConfig {
                    schedule,
                    announcement_offset: offset,
                });
        let cli_pairs = self.pair;

        let (file_schedule, file_pairs) = (cli_schedule.is_none() || cli_pairs.is_none())
            .then(|| {
                let path = self.config_file.unwrap_or("config.json".into());
                info!(
                    "reading asset pair and oracle scheduler config from {}",
                    path.as_os_str().to_string_lossy()
                );
                let mut buffer_read = String::new();
                File::open(path)?.read_to_string(&mut buffer_read)?;
                let ConfigurationFile {
                    pairs,
                    oracle_scheduler_config,
                } = serde_json::from_str(&buffer_read)?;
                Ok::<_, PythiaConfigError>((oracle_scheduler_config, pairs))
            })
            .transpose()?
            .unzip();

        let (asset_pair_infos, oracle_scheduler_config) = (
            cli_pairs
                .or(file_pairs)
                .expect("file loaded if cli missing"),
            cli_schedule
                .or(file_schedule)
                .expect("file loaded if cli missing"),
        );

        info!(
            "asset pair and oracle scheduler config successfully read: {:#?}\n{}",
            &asset_pair_infos, &oracle_scheduler_config
        );

        let db_connect = self.url_postgres.unwrap_or(match_postgres_env()?);

        let secret_key = match self.secret_key {
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

fn parse_asset_pair_array(val: &str) -> Result<AssetPairInfo, String> {
    let rules = serde_json::from_str(val).map_err(|e| e.to_string())?;
    Ok(rules)
}

fn parse_duration(v: &str) -> Result<chrono::Duration, String> {
    Ok(Duration::nanoseconds(
        humantime::parse_duration(v)
            .map_err(|ref e| e.to_string())?
            .as_nanos()
            .try_into()
            .map_err(|e: <u128 as TryFrom<i64>>::Error| e.to_string())?,
    ))
}
