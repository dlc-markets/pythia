use std::{
    fs::{self, File},
    io::Read,
    str::FromStr,
};

use clap::Parser;

use crate::common::{AssetPairInfo, ConfigurationFile, OracleSchedulerConfig};

use sqlx::postgres::PgConnectOptions;

use anyhow::Result;

#[derive(Parser)]
/// Simple DLC oracle implementation
pub struct PythiaArgs {
    /// Optional private key file; if not provided, one is generated
    #[clap(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    pub secret_key_file: Option<std::path::PathBuf>,

    /// Optional config file; if not provided, it is assumed to exist at "config.json"
    #[clap(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    config_file: Option<std::path::PathBuf>,

    /// Optional port for API; if not provided, use 8000
    #[clap(short, long, value_name = "PORT")]
    port: Option<u16>,

    /// Optional postgres URL for oracle DB, if not provided use "postgres://postgres:postgres@127.0.0.1:5432/oracle"
    #[clap(short, long, value_name = "URL", value_hint = clap::ValueHint::Url)]
    url_postgres: Option<PgConnectOptions>,

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
        let config_file: ConfigurationFile = match self.config_file {
            None => {
                info!("reading asset pair and oracle scheduler config from config.json");
                serde_json::from_str(&fs::read_to_string("config.json")?)?
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
            "asset pair and oracle scheduler config successfully read: {:#?}",
            (&asset_pair_infos, &oracle_scheduler_config)
        );

        let port: u16 = self.port.unwrap_or(8000);

        match self.url_postgres {
            None => {
                let db_connect = PgConnectOptions::from_str(
                    "postgres://postgres:postgres@127.0.0.1:5432/oracle",
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
