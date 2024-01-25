use std::{env::var, str::FromStr};

use secp256k1_zkp::SecretKey;
use sqlx::postgres::PgConnectOptions;

use super::error::PythiaConfigError;

pub(super) fn match_postgres_env() -> Result<PgConnectOptions, PythiaConfigError> {
    let pg_password = var("POSTGRES_PASSWORD").unwrap_or("postgres".to_owned());
    let pg_user = var("POSTGRES_USER").unwrap_or("postgres".to_owned());
    let pg_db = var("POSTGRES_DB").unwrap_or("postgres".to_owned());
    let pg_host = var("POSTGRES_HOST").unwrap_or("localhost".to_owned());
    let pg_port = var("POSTGRES_PORT")
        .unwrap_or("5432".to_owned())
        .parse::<u16>()?;

    Ok(PgConnectOptions::new()
        .database(&pg_db)
        .port(pg_port)
        .host(&pg_host)
        .username(&pg_user)
        .password(&pg_password))
}

pub(super) fn match_secret_key_env() -> Result<SecretKey, PythiaConfigError> {
    let secret_key = var("PYTHIA_SECRET_KEY").map_err(|_e| PythiaConfigError::NoSecretKey)?;
    SecretKey::from_str(secret_key.as_str()).map_err(|_e| PythiaConfigError::InvalidSecretKey)
}

pub(super) fn match_debug_mode_env() -> bool {
    let debug_mode = var("PYTHIA_DEBUG_MODE").unwrap_or("false".to_owned());
    bool::from_str(debug_mode.as_str()).unwrap_or(false)
}

pub(super) fn match_port_env() -> u16 {
    let port = var("PYTHIA_PORT").unwrap_or("8000".to_owned());
    u16::from_str(port.as_str()).unwrap_or(8000)
}

pub(super) fn match_nb_connection() -> u32 {
    let max_connections = var("PYTHIA_NB_CONNECTION").unwrap_or("10".to_owned());
    u32::from_str(max_connections.as_str()).unwrap_or(10)
}
