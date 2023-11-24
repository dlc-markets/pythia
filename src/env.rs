use std::{env, str::FromStr};

use secp256k1_zkp::SecretKey;
use sqlx::postgres::PgConnectOptions;

use crate::error::PythiaError;

pub(super) fn match_postgres_env() -> Result<PgConnectOptions, PythiaError> {
    let pg_password = env::var("POSTGRES_PASSWORD").unwrap_or("postgres".to_owned());
    let pg_user = env::var("POSTGRES_USER").unwrap_or("postgres".to_owned());
    let pg_db = env::var("POSTGRES_DB").unwrap_or("postgres".to_owned());
    let pg_host = env::var("POSTGRES_HOST").unwrap_or("localhost".to_owned());
    let pg_port = env::var("POSTGRES_PORT")
        .unwrap_or("5432".to_owned())
        .parse::<u16>()?;

    Ok(PgConnectOptions::new()
        .database(&pg_db)
        .port(pg_port)
        .host(&pg_host)
        .username(&pg_user)
        .password(&pg_password))
}

pub(super) fn match_secret_key_env() -> Result<SecretKey, PythiaError> {
    let secret_key = env::var("PYTHIA_SECRET_KEY").map_err(|_e| PythiaError::NoSecretKey)?;
    SecretKey::from_str(secret_key.as_str()).map_err(|_e| PythiaError::InvalidSecretKey)
}

pub(super) fn match_debug_mode_env() -> bool {
    let debug_mode = env::var("PYTHIA_DEBUG_MODE").unwrap_or("false".to_owned());
    bool::from_str(debug_mode.as_str()).unwrap_or(false)
}

pub(super) fn match_port_env() -> u16 {
    let port = env::var("PYTHIA_PORT").unwrap_or("8000".to_owned());
    u16::from_str(port.as_str()).unwrap_or(8000)
}

pub(super) fn match_nb_connection() -> u32 {
    let max_connections = env::var("PYTHIA_NB_CONNECTION").unwrap_or("10".to_owned());
    u32::from_str(max_connections.as_str()).unwrap_or(8000)
}
