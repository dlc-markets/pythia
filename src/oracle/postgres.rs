use chrono::{DateTime, Utc};
use dlc_messages::oracle_msgs::{EventDescriptor, OracleAnnouncement, OracleAttestation};
use secp256k1_zkp::{schnorr::Signature, Scalar, XOnlyPublicKey};
use sqlx::{
    postgres::{PgConnectOptions, PgPool, PgPoolOptions},
    Result,
};

struct EventResponse {
    digits: i32,
    precision: i32,
    maturity: DateTime<Utc>,
    announcement_signature: Vec<u8>,
    outcome: Option<f64>,
}

struct DigitAnnouncementResponse {
    nonce_public: Vec<u8>,
    nonce_secret: Option<Vec<u8>>,
}
struct DigitAttestationResponse {
    nonce_public: Vec<u8>,
    signature: Option<Vec<u8>>,
}

#[derive(Clone)]
pub(super) enum ScalarsRecords {
    DigitsSkNonce(Vec<[u8; 32]>),
    DigitsAttestations(f64, Vec<Scalar>),
}

#[derive(Clone)]
pub(super) struct PostgresResponse {
    pub digits: u16,
    pub precision: u16,
    pub maturity: DateTime<Utc>,
    pub announcement_signature: Signature,
    pub nonce_public: Vec<XOnlyPublicKey>,
    pub scalars_records: ScalarsRecords,
}
#[derive(Clone)]
pub(crate) struct DBconnection(pub PgPool);

impl DBconnection {
    /// Create a new Db connection with postgres
    pub(crate) async fn new(db_connect: PgConnectOptions, max_connection: u32) -> Result<Self> {
        Ok(DBconnection(
            PgPoolOptions::new()
                .max_connections(max_connection)
                .connect_with(db_connect)
                .await?,
        ))
    }

    pub(crate) async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.0).await?;
        Ok(())
    }

    pub(super) async fn is_empty(&self) -> bool {
        sqlx::query_as!(EventResponse, "SELECT digits, precision, maturity, announcement_signature, outcome FROM oracle.events LIMIT 1")
            .fetch_optional(&self.0)
            .await
            .unwrap()
            .is_none()
    }

    /// Insert announcement data and meta-data in postgres DB
    pub(super) async fn insert_announcement(
        &self,
        announcement: &OracleAnnouncement,
        outstanding_sk_nonces: Vec<[u8; 32]>,
    ) -> Result<()> {
        let EventDescriptor::DigitDecompositionEvent(ref digits) =
            announcement.oracle_event.event_descriptor
        else {
            return Err(sqlx::Error::TypeNotFound {
                type_name: "Only DigitDecomposition event type is supported".to_string(),
            });
        };

        let sk_nonces = outstanding_sk_nonces
            .into_iter()
            .map(|x| x.into())
            .collect::<Vec<_>>();

        let query_result = sqlx::query!(
            "WITH events AS (
                INSERT INTO oracle.events VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING
            )
            INSERT INTO oracle.digits (event_id, digit_index, nonce_public, nonce_secret) (
                SELECT * FROM UNNEST($6::VARCHAR[], $7::INT[], $8::BYTEA[], $9::BYTEA[])
            ) ON CONFLICT DO NOTHING
            ",
            &announcement.oracle_event.event_id,
            &(digits.nb_digits as i32),
            digits.precision,
            DateTime::from_timestamp(announcement.oracle_event.event_maturity_epoch.into(), 0)
                .unwrap(),
            announcement.announcement_signature.as_ref(),
            &vec![announcement.oracle_event.event_id.to_owned(); digits.nb_digits as usize][..],
            &(0..digits.nb_digits as i32).collect::<Vec<i32>>(),
            &announcement
                .oracle_event
                .oracle_nonces
                .iter()
                .map(|x| x.serialize().to_vec())
                .collect::<Vec<Vec<u8>>>(),
            sk_nonces.as_slice()
        )
        .execute(&self.0)
        .await?;

        let affected_raws_counts = query_result.rows_affected();

        trace!(
            "Rows affected count when inserting announcement: {}",
            affected_raws_counts
        );

        if affected_raws_counts != digits.nb_digits as u64 {
            match affected_raws_counts {
                0 => warn!("We tried to insert an announcement while it was already in postgres database. This is normal only if another pythia instance is running using the same database with the same private key."),
                x @ 1.. => error!("Updating to attestation did not affect the expected number of rows: {} instead of {}", x, digits.nb_digits),
            }
        }

        Ok(())
    }

    /// Add signed outcome to meta data and digits signatures to DB and delete secret nonces to avoid secret key leaking
    pub(super) async fn update_to_attestation(
        &self,
        event_id: &str,
        attestation: &OracleAttestation,
        outcome: f64,
    ) -> Result<()> {
        let (indexes, sigs): (Vec<usize>, Vec<Vec<u8>>) = attestation
            .signatures
            .iter()
            .map(|sig| sig.as_ref().split_at(32).1.to_vec())
            .enumerate()
            .unzip();

        // SECURITY: secret nonce MUST be dropped from DB by setting all of them to null.
        // This ensures that a DB leakage would not immediately allow secret key extraction
        // Notice: secret key is still leaked if we sign events which secret nonce was in leaked DB
        let query_result = sqlx::query!(
            "WITH events AS (
                UPDATE oracle.events SET outcome = $1::FLOAT8 WHERE id = $2::TEXT
            )
            UPDATE oracle.digits
        SET signature = bulk.sig, signing_ts = NOW(), nonce_secret = NULL
        FROM ( 
            SELECT *
            FROM UNNEST($3::BYTEA[], $4::VARCHAR[], $5::INT[]) 
            AS t(sig, id, digit)
            ) AS bulk 
        WHERE event_id = bulk.id AND digit_index = bulk.digit AND nonce_secret IS NOT NULL
        ",
            outcome,
            event_id,
            &sigs,
            &vec![event_id.to_owned(); indexes.len() as usize][..],
            &indexes.into_iter().map(|x| x as i32).collect::<Vec<i32>>()[..],
        )
        .execute(&self.0)
        .await?;

        let affected_raws_counts = query_result.rows_affected();

        trace!(
            "Rows affected count when updating to attestation: {}",
            affected_raws_counts
        );

        if affected_raws_counts as usize != sigs.len() {
            match affected_raws_counts {
                0 => warn!("We tried to update an announcement into an attestation while it was already an attestation in postgres database. This is normal only if another pythia instance is running using the same database with the same private key."),
                x @ 1.. => error!("Updating to attestation did not affect the expected number of rows: {} instead of {}", x, sigs.len()),
            }
        }

        Ok(())
    }

    /// Retrieve the current state of an event in oracle's DB
    pub(super) async fn get_event(&self, event_id: &str) -> Result<Option<PostgresResponse>> {
        let Some(event) = sqlx::query_as!(
        EventResponse,
        "SELECT digits, precision, maturity, announcement_signature, outcome FROM oracle.events e WHERE e.id = $1",
        event_id
    )
    .fetch_optional(&self.0)
    .await? else {return Ok(None)};

        match event.outcome {
            None => {
                let digits = sqlx::query_as!(
                DigitAnnouncementResponse,
                "SELECT nonce_public, nonce_secret FROM oracle.digits WHERE event_id = $1 ORDER BY digit_index;",
                event_id
            )
            .fetch_all(&self.0)
            .await?;
                let aggregated_rows: Vec<(XOnlyPublicKey, [u8; 32])> = digits
                    .iter()
                    .map(|x| {
                        (
                            XOnlyPublicKey::from_slice(&x.nonce_public).unwrap(),
                            x.nonce_secret
                                .as_ref()
                                .unwrap()
                                .as_slice()
                                .try_into()
                                .unwrap(),
                        )
                    })
                    .collect();
                let (nonce_public, nonce_secret): (Vec<XOnlyPublicKey>, Vec<[u8; 32]>) =
                    aggregated_rows.into_iter().unzip();
                Ok(Some(PostgresResponse {
                    digits: event.digits as u16,
                    precision: event.precision as u16,
                    maturity: event.maturity,
                    announcement_signature: Signature::from_slice(
                        &event.announcement_signature[..],
                    )
                    .unwrap(),
                    nonce_public,
                    scalars_records: ScalarsRecords::DigitsSkNonce(nonce_secret),
                }))
            }
            Some(outcome) => {
                let digits = sqlx::query_as!(
                DigitAttestationResponse,
                "SELECT nonce_public, signature FROM oracle.digits WHERE event_id = $1 ORDER BY digit_index;",
                event_id
            )
            .fetch_all(&self.0)
            .await?;
                type AggregatedRows = (XOnlyPublicKey, Scalar);
                let aggregated_rows: Vec<AggregatedRows> = digits
                    .iter()
                    .map(|x| {
                        (
                            XOnlyPublicKey::from_slice(&x.nonce_public).unwrap(),
                            Scalar::from_be_bytes(
                                x.signature.as_ref().unwrap().as_slice().try_into().unwrap(),
                            )
                            .unwrap(),
                        )
                    })
                    .collect();
                let (nonce_public, sigs): (Vec<XOnlyPublicKey>, Vec<Scalar>) =
                    aggregated_rows.into_iter().unzip();
                Ok(Some(PostgresResponse {
                    digits: event.digits as u16,
                    precision: event.precision as u16,
                    maturity: event.maturity,
                    announcement_signature: Signature::from_slice(
                        &event.announcement_signature[..],
                    )
                    .unwrap(),
                    nonce_public,
                    scalars_records: ScalarsRecords::DigitsAttestations(outcome, sigs),
                }))
            }
        }
    }
}
