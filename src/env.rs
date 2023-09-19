use std::env;

use sqlx::postgres::PgConnectOptions;

pub(super) fn match_postgres_env() -> Option<PgConnectOptions> {
    let pg_password = env::var("POSTGRES_PASSWORD").ok();
    let pg_user = env::var("POSTGRES_USER").ok();
    let pg_db = env::var("POSTGRES_DB").ok();
    let pg_host = env::var("POSTGRES_HOST").ok();
    let pg_port = env::var("POSTGRES_PORT").ok()?.parse::<u16>().ok();

    pg_password
        .zip(pg_user)
        .zip(pg_host)
        .zip(pg_port)
        .zip(pg_db)
        .map(|x| {
            PgConnectOptions::new()
                .database(&x.1)
                .port(x.0 .1)
                .host(&x.0 .0 .1)
                .username(&x.0 .0 .0 .1)
                .password(&x.0 .0 .0 .0)
        })
}
