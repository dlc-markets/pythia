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
    let secret_key = env::var("ORACLE_SECRET_KEY").map_err(|_e| PythiaError::NoSecretKey)?;
    SecretKey::from_str(secret_key.as_str()).map_err(|_e| PythiaError::InvalidSecretKey)
}
