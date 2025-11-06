use chrono::{DateTime, Utc};
use secp256k1_zkp::{Scalar, schnorr::Signature};
use sqlx::{
    Result,
    postgres::{PgConnectOptions, PgPool, PgPoolOptions},
};

use crate::data_models::event_ids::EventIdInfos;

#[derive(Clone, Debug)]
pub(super) enum ScalarsRecords {
    DigitsSkNonce(Box<[[u8; 32]]>),
    DigitsAttestations(f64, Box<[Scalar]>),
}
#[derive(Clone, Debug)]
pub(crate) struct EventFromPostgres {
    pub digits: u16,
    pub precision: u16,
    pub event_id_infos: EventIdInfos,
    pub maturity: DateTime<Utc>,
    pub announcement_signature: Signature,
    pub nonces_public: Box<[[u8; 32]]>,
    pub scalars_records: ScalarsRecords,
}
#[derive(Clone)]
pub(super) struct DBconnection(pub PgPool);

impl DBconnection {
    /// Create a new Db connection with postgres
    pub async fn new(db_connect: PgConnectOptions, max_connection: u32) -> Result<Self> {
        Ok(DBconnection(
            PgPoolOptions::new()
                .max_connections(max_connection)
                .connect_with(db_connect)
                .await?,
        ))
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.0).await?;
        Ok(())
    }

    pub async fn is_empty(&self) -> Result<bool> {
        sqlx::query_scalar!(
            r#"SELECT NOT EXISTS (SELECT 1 FROM oracle.events LIMIT 1) AS "exist!" "#
        )
        .fetch_one(&self.0)
        .await
    }
}
