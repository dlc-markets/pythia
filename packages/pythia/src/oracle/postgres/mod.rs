use chrono::{DateTime, Utc};
use dlc_messages::oracle_msgs::{EventDescriptor, OracleAnnouncement, OracleAttestation};
use secp256k1_zkp::{schnorr::Signature, Scalar, XOnlyPublicKey};
use sqlx::{
    postgres::{PgConnectOptions, PgPool, PgPoolOptions},
    Result,
};

use std::io::{Error, ErrorKind};

use super::OracleAnnouncementWithSkNonces;

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
#[derive(Default)]
struct BatchedAnnouncementResponse {
    digits: i32,
    precision: i32,
    maturity: DateTime<Utc>,
    announcement_signature: Vec<u8>,
    nonces_public: Option<Vec<Vec<u8>>>,
}
struct DigitAttestationResponse {
    nonce_public: Vec<u8>,
    signature: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub(super) enum ScalarsRecords {
    DigitsSkNonce(Vec<[u8; 32]>),
    DigitsAttestations(f64, Vec<Scalar>),
}

#[derive(Clone)]
struct BoolResponse {
    all_exist: Option<bool>,
}

#[derive(PartialEq, PartialOrd)]
pub(super) struct MaturityResponse {
    pub maturity: DateTime<Utc>,
}

#[derive(Clone, Debug)]
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

    pub(super) async fn is_empty(&self) -> Result<bool> {
        Ok(sqlx::query_as!(EventResponse, "SELECT digits, precision, maturity, announcement_signature, outcome FROM oracle.events LIMIT 1")
            .fetch_optional(&self.0)
            .await?
            .is_none())
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
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "Only DigitDecomposition event type is supported",
            ))?;
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
                .ok_or(Error::new(
                    ErrorKind::InvalidInput,
                    "Failed to convert timestamp to DateTime"
                ))?,
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

        trace!("Rows affected count when inserting announcement: {affected_raws_counts}");

        if affected_raws_counts != digits.nb_digits as u64 {
            match affected_raws_counts {
                0 => warn!("We tried to insert an announcement while it was already in postgres database. This is normal only if another pythia instance is running using the same database with the same private key."),
                x @ 1.. => error!("Updating to attestation did not affect the expected number of rows: {} instead of {}", x, digits.nb_digits),
            }
        }

        Ok(())
    }

    /// Insert many announcements in postgres DB.
    /// The announcements must be sorted by event id,
    /// otherwise it panics before the insert
    pub(super) async fn insert_many_announcements(
        &self,
        announcements_with_sk_nonces_sorted_by_id: &[OracleAnnouncementWithSkNonces],
    ) -> Result<()> {
        if announcements_with_sk_nonces_sorted_by_id.is_empty() {
            return Ok(());
        }

        // Sanity check that announcements are sorted
        // if not we panic to not corrupt the database
        assert!(
            announcements_with_sk_nonces_sorted_by_id.is_sorted_by(|a, b| a
                .0
                .oracle_event
                .event_id
                .as_str()
                <= b.0.oracle_event.event_id.as_str()),
            "announcements_with_sk_nonces_sorted_by_id must be sorted in ascending order before inserting into database"
        );

        #[allow(clippy::type_complexity)]
        let (
            event_ids,
            digits_counts,
            precisions,
            maturities,
            announcement_signatures,
            nonce_publics,
            nonce_secrets,
        ): (
            Vec<String>,
            Vec<i32>,
            Vec<i32>,
            Vec<DateTime<Utc>>,
            Vec<Vec<u8>>,
            Vec<_>,
            Vec<_>,
        ) = announcements_with_sk_nonces_sorted_by_id
            .iter()
            .map(|(announcement, outstanding_sk_nonces)| {
                let EventDescriptor::DigitDecompositionEvent(ref digit) =
                    announcement.oracle_event.event_descriptor
                else {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        "Only DigitDecomposition event type is supported",
                    ))?;
                };
                Ok((
                    announcement.oracle_event.event_id.clone(),
                    digit.nb_digits as i32,
                    digit.precision,
                    DateTime::from_timestamp(
                        announcement.oracle_event.event_maturity_epoch.into(),
                        0,
                    )
                    .ok_or(Error::new(
                        ErrorKind::InvalidInput,
                        "Failed to convert timestamp to DateTime",
                    ))?,
                    announcement.announcement_signature.as_ref().to_vec(),
                    announcement
                        .oracle_event
                        .oracle_nonces
                        .iter()
                        .map(|x| x.serialize().to_vec()),
                    outstanding_sk_nonces.iter().map(|x| x.to_vec()),
                ))
            })
            .collect::<Result<_>>()?;

        let query_result = sqlx::query!(
            "WITH events AS (
                INSERT INTO oracle.events (id, digits, precision, maturity, announcement_signature) 
                SELECT * FROM UNNEST($1::VARCHAR[], $2::INT[], $3::INT[], $4::TIMESTAMPTZ[], $5::BYTEA[])
                ON CONFLICT DO NOTHING
                RETURNING id, digits
            ),
            events_with_offset AS (
                SELECT id, digits,
                       SUM(digits) OVER (ORDER BY id) - digits as prev_sum
                FROM events
            ),
            nonces_arrays AS (
                SELECT array_agg(nonce_public) as nonce_publics,
                       array_agg(nonce_secret) as nonce_secrets
                FROM UNNEST($6::BYTEA[], $7::BYTEA[]) as t(nonce_public, nonce_secret)
            )
            INSERT INTO oracle.digits (event_id, digit_index, nonce_public, nonce_secret)
            SELECT 
                e.id,
                g.digit_index,
                (SELECT nonce_publics[e.prev_sum + g.digit_index + 1] FROM nonces_arrays),
                (SELECT nonce_secrets[e.prev_sum + g.digit_index + 1] FROM nonces_arrays)
            FROM events_with_offset e
            CROSS JOIN LATERAL generate_series(0, e.digits - 1) as g(digit_index)
            ON CONFLICT DO NOTHING
            ",
            &event_ids,
            &digits_counts,
            &precisions,
            &maturities,
            &announcement_signatures,
            &nonce_publics.into_iter().flatten().collect::<Vec<_>>(),
            &nonce_secrets.into_iter().flatten().collect::<Vec<_>>(),
        ).execute(&self.0).await?;

        let affected_raws_counts = query_result.rows_affected();

        trace!("Rows affected count when inserting announcement: {affected_raws_counts}");
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

        trace!("Rows affected count when updating to attestation: {affected_raws_counts}");

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
        "SELECT digits, precision, maturity, announcement_signature, outcome FROM oracle.events WHERE id = $1",
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
                            XOnlyPublicKey::from_slice(&x.nonce_public)
                                .expect("nonce_public must have valid length inserted by pythia"),
                            x.nonce_secret
                                .as_ref()
                                .expect("nonce_secret must be present in digits table if we did not get an attestation in events table")
                                .as_slice()
                                .try_into()
                                .expect("nonce_secret must have valid length inserted by pythia"),
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
                    .expect("announcement_signature must have valid length inserted by pythia"),
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
                            XOnlyPublicKey::from_slice(&x.nonce_public)
                                .expect("pythia must have inserted nonce of valid length"),
                            Scalar::from_be_bytes(
                                x.signature
                                    .as_deref()
                                    .expect("signature must be present in digits table if we got an outcome in events table")
                                    .try_into()
                                    .expect("signature length inserted by pythia is correct"),
                            )
                            .expect("pythia inserted valid scalar values in digits table"),
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
                    .expect("announcement_signature must have valid length inserted by pythia"),
                    nonce_public,
                    scalars_records: ScalarsRecords::DigitsAttestations(outcome, sigs),
                }))
            }
        }
    }

    /// Retrieve the current state of many events in oracle's DB
    pub(super) async fn get_many_events(
        &self,
        mut events_ids: Vec<String>,
    ) -> Result<Option<Vec<PostgresResponse>>> {
        // Remove duplicate maturities
        events_ids.sort_unstable();
        events_ids.dedup();

        let BoolResponse { all_exist } = sqlx::query_as!(
            BoolResponse,
            "SELECT COUNT(*) = COALESCE ($2, 0) AS all_exist
            FROM (
                SELECT DISTINCT UNNEST($1::VARCHAR[]) AS id
            ) AS distinct_event_id 
            WHERE id IN (SELECT id FROM oracle.events);",
            &events_ids,
            events_ids.len() as i64
        )
        .fetch_one(&self.0)
        .await?;

        if !all_exist.expect("cannot be null because of coalesce") {
            return Ok(None);
        };

        let batch = sqlx::query_as!(
            BatchedAnnouncementResponse,
            "SELECT 
                e.digits, 
                e.precision, 
                e.maturity, 
                e.announcement_signature, 
                COALESCE(
                    array_agg(
                        d.nonce_public 
                    ORDER BY d.digit_index
                        ), 
                    '{}') AS nonces_public 
            FROM 
                oracle.events e 
            LEFT JOIN 
                oracle.digits d 
            ON 
                e.id = d.event_id 
            WHERE 
                d.event_id = ANY ($1::VARCHAR[]) 
            GROUP BY 
                e.id, e.digits, e.precision, e.maturity, e.announcement_signature
            ORDER BY
                e.id;",
            &events_ids
        )
        .fetch_all(&self.0)
        .await?;

        Ok(Some(
            batch
                .into_iter()
                .map(
                    |BatchedAnnouncementResponse {
                         digits,
                         precision,
                         maturity,
                         announcement_signature,
                         nonces_public,
                     }| {
                        let nonces_public =
                            nonces_public.expect("COALESCE in psql query guarantee it is not None");
                        PostgresResponse {
                            digits: digits as u16,
                            precision: precision as u16,
                            maturity,
                            announcement_signature: Signature::from_slice(
                                &announcement_signature[..],
                            )
                            .expect(
                                "announcement_signature must have valid length inserted by pythia",
                            ),
                            nonce_public: nonces_public
                                .into_iter()
                                .map(|ref s| {
                                    XOnlyPublicKey::from_slice(s).expect(
                                        "nonce_public must have valid length inserted by pythia",
                                    )
                                })
                                .collect(),
                            scalars_records: ScalarsRecords::DigitsSkNonce(Vec::new()),
                        }
                    },
                )
                .collect(),
        ))
    }

    pub(super) async fn get_non_existing_sorted_maturity(
        &self,
        maturities: &[DateTime<Utc>],
    ) -> Result<Vec<DateTime<Utc>>> {
        let maturity_response = sqlx::query_as!(
            MaturityResponse,
            r#"
            WITH maturity_array AS (
                SELECT maturity FROM UNNEST($1::TIMESTAMPTZ[]) as maturity
            )
            SELECT
               maturity as "maturity!"
            FROM
                maturity_array
            WHERE NOT EXISTS (
                SELECT maturity
                FROM oracle.events
                WHERE oracle.events.maturity = maturity_array.maturity
            )
            ORDER BY maturity
            ;"#,
            &maturities
        )
        .fetch_all(&self.0)
        .await?;
        let maturity_array = maturity_response.into_iter().map(|x| x.maturity).collect();
        Ok(maturity_array)
    }
}

#[cfg(test)]
mod test;
