use chrono::{DateTime, Utc};
use secp256k1_zkp::{schnorr::Signature, Scalar};
use sqlx::{
    postgres::{PgConnectOptions, PgPool, PgPoolOptions},
    prelude::FromRow,
    Result,
};

use std::io::{Error, ErrorKind};

use crate::{
    data_models::{event_ids::EventId, oracle_msgs::Attestation},
    oracle::SignedEventToInsert,
};

#[derive(FromRow)]
struct EventResponse {
    digits: i32,
    precision: i32,
    maturity: DateTime<Utc>,
    announcement_signature: Vec<u8>,
    outcome: Option<f64>,
}

#[derive(FromRow)]
struct DigitAnnouncementResponse {
    nonce_public: Vec<u8>,
    nonce_secret: Option<Vec<u8>>,
}
#[derive(Default, FromRow)]
struct BatchedAnnouncementResponse {
    digits: i32,
    precision: i32,
    maturity: DateTime<Utc>,
    announcement_signature: Vec<u8>,
    nonces_public: Option<Vec<[u8; 32]>>,
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

#[derive(FromRow)]
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
    pub nonces_public: Vec<[u8; 32]>,
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
    pub(super) async fn insert_announcement(&self, event: &SignedEventToInsert) -> Result<()> {
        let digits = event.event_descriptor;

        let (nonce_publics, nonce_secrets) = event
            .nonces_keypairs
            .iter()
            .map(|kp| (kp.x_only_public_key().0.serialize(), kp.secret_bytes()))
            .collect::<(Vec<[u8; 32]>, Vec<[u8; 32]>)>();

        let query_result = sqlx::query(
            "WITH events AS (
                INSERT INTO oracle.events VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING
            )
            INSERT INTO oracle.digits (event_id, digit_index, nonce_public, nonce_secret) (
                SELECT * FROM UNNEST($6::VARCHAR[], $7::INT[], $8::BYTEA[], $9::BYTEA[])
            ) ON CONFLICT DO NOTHING
            ",
        )
        .bind(event.event_id)
        .bind(digits.nb_digits as i32)
        .bind(digits.precision)
        .bind(
            DateTime::from_timestamp(event.maturity.into(), 0).ok_or(Error::new(
                ErrorKind::InvalidInput,
                "Failed to convert timestamp to DateTime",
            ))?,
        )
        .bind(event.announcement_signature.as_ref())
        .bind(vec![event.event_id; digits.nb_digits as usize])
        .bind((0..digits.nb_digits as i32).collect::<Vec<i32>>())
        .bind(&nonce_publics)
        .bind(&nonce_secrets)
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
        events_sorted_by_id: &[SignedEventToInsert],
    ) -> Result<()> {
        let Some(first_event) = events_sorted_by_id.first() else {
            return Ok(());
        };

        // Sanity check that announcements are sorted
        // if not we panic to not corrupt the database
        assert!(
            events_sorted_by_id.is_sorted_by(|a, b| a
                .event_id
                <= b.event_id
            ),
            "announcements_sorted_by_id must be sorted in ascending order before inserting into database"
        );

        let estimated_nb_digits = first_event.event_descriptor.nb_digits as usize;

        let mut tuple_extend = (
            Vec::with_capacity(events_sorted_by_id.len() * estimated_nb_digits),
            Vec::with_capacity(events_sorted_by_id.len() * estimated_nb_digits),
        );

        let (event_ids, digits_counts, precisions, maturities, announcement_signatures) =
            events_sorted_by_id
                .iter()
                .map(|event| {
                    tuple_extend.extend(
                        event
                            .nonces_keypairs
                            .iter()
                            .map(|kp| (kp.x_only_public_key().0.serialize(), kp.secret_bytes())),
                    );

                    Ok((
                        event.event_id,
                        event.event_descriptor.nb_digits as i32,
                        event.event_descriptor.precision,
                        DateTime::from_timestamp(event.maturity.into(), 0).ok_or(Error::new(
                            ErrorKind::InvalidInput,
                            "Failed to convert timestamp to DateTime",
                        ))?,
                        event.announcement_signature.as_ref(),
                    ))
                })
                .collect::<Result<(
                    Vec<EventId>,
                    Vec<i32>,
                    Vec<i32>,
                    Vec<DateTime<Utc>>,
                    Vec<&[u8; 64]>,
                )>>()?;

        let (nonce_publics, nonce_secrets) = tuple_extend;

        let query_result = sqlx::query(
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
            ")
            .bind(event_ids)
            .bind(digits_counts)
            .bind(precisions)
            .bind(maturities)
            .bind(announcement_signatures)
            .bind(nonce_publics)
            .bind(nonce_secrets)
            .execute(&self.0)
            .await?;

        let affected_raws_counts = query_result.rows_affected();

        trace!("Rows affected count when inserting announcement: {affected_raws_counts}");
        Ok(())
    }

    /// Add signed outcome to meta data and digits signatures to DB and delete secret nonces to avoid secret key leaking
    pub(super) async fn update_to_attestation(
        &self,
        event_id: EventId,
        attestation: &Attestation,
        outcome: f64,
    ) -> Result<()> {
        let (indexes, sigs) = attestation
            .signatures
            .iter()
            .enumerate()
            .map(|(index, sig)| (index as i32, sig.as_ref().split_at(32).1))
            .collect::<(Vec<_>, Vec<_>)>();

        // SECURITY: secret nonce MUST be dropped from DB by setting all of them to null.
        // This ensures that a DB leakage would not immediately allow secret key extraction
        // Notice: secret key is still leaked if we sign events which secret nonce was in leaked DB
        let query_result = sqlx::query(
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
        )
        .bind(outcome)
        .bind(event_id)
        .bind(sigs.as_slice())
        .bind(vec![event_id; indexes.len()])
        .bind(indexes)
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
    pub(super) async fn get_event(&self, event_id: EventId) -> Result<Option<PostgresResponse>> {
        let Some(event) = sqlx::query_as!(
            EventResponse,
            "SELECT digits, precision, maturity, announcement_signature, outcome FROM oracle.events WHERE id = $1",
            event_id.as_ref()
        )
    .fetch_optional(&self.0)
    .await? else {return Ok(None)};

        match event.outcome {
            None => {
                let digits = sqlx::query_as!(
                DigitAnnouncementResponse,
                "SELECT nonce_public, nonce_secret FROM oracle.digits WHERE event_id = $1 ORDER BY digit_index;",
                event_id.as_ref()
            )
            .fetch_all(&self.0)
            .await?;
                let (nonces_public, nonce_secret) = digits
                    .iter()
                    .map(|x| {
                        (
                            <[u8; 32]>::try_from(&x.nonce_public[..])
                                .expect("nonce_public must have valid length inserted by pythia"),
                            <[u8; 32]>::try_from(
                                x.nonce_secret
                                    .as_ref()
                                    .expect("nonce_secret must be present in digits table if we did not get an attestation in events table")
                                    .as_slice(),
                            )
                                .expect("nonce_secret must have valid length inserted by pythia"),
                        )
                    })
                    .collect::<(Vec<_>, Vec<_>)>();
                Ok(Some(PostgresResponse {
                    digits: event.digits as u16,
                    precision: event.precision as u16,
                    maturity: event.maturity,
                    announcement_signature: Signature::from_slice(
                        &event.announcement_signature[..],
                    )
                    .expect("announcement_signature must have valid length inserted by pythia"),
                    nonces_public,
                    scalars_records: ScalarsRecords::DigitsSkNonce(nonce_secret),
                }))
            }
            Some(outcome) => {
                let digits = sqlx::query_as!(
                DigitAttestationResponse,
                "SELECT nonce_public, signature FROM oracle.digits WHERE event_id = $1 ORDER BY digit_index;",
                event_id.as_ref()
            )
            .fetch_all(&self.0)
            .await?;

                let (nonces_public, sigs) = digits
                    .iter()
                    .map(|x| {
                        (
                            <[u8; 32]>::try_from(&x.nonce_public[..])
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
                Ok(Some(PostgresResponse {
                    digits: event.digits as u16,
                    precision: event.precision as u16,
                    maturity: event.maturity,
                    announcement_signature: Signature::from_slice(
                        &event.announcement_signature[..],
                    )
                    .expect("announcement_signature must have valid length inserted by pythia"),
                    nonces_public,
                    scalars_records: ScalarsRecords::DigitsAttestations(outcome, sigs),
                }))
            }
        }
    }

    /// Retrieve the current state of many events in oracle's DB
    pub(super) async fn get_many_events(
        &self,
        mut events_ids: Vec<EventId>,
    ) -> Result<Option<Vec<(PostgresResponse, EventId)>>> {
        // Remove duplicate maturities
        events_ids.sort_unstable();
        events_ids.dedup();

        let BoolResponse { all_exist } = sqlx::query_as(
            "SELECT COUNT(*) = COALESCE ($2, 0) AS all_exist
            FROM (
                SELECT DISTINCT UNNEST($1::TEXT[]) AS id
            ) AS distinct_event_id 
            WHERE id IN (SELECT id FROM oracle.events);",
        )
        .bind(&events_ids)
        .bind(events_ids.len() as i64)
        .fetch_one(&self.0)
        .await?;

        if !all_exist.expect("cannot be null because of coalesce") {
            return Ok(None);
        };

        let batch: Vec<BatchedAnnouncementResponse> = sqlx::query_as(
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
        )
        .bind(&events_ids)
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
                            nonces_public,
                            scalars_records: ScalarsRecords::DigitsSkNonce(Vec::new()),
                        }
                    },
                )
                .zip(events_ids)
                .collect::<Vec<_>>(),
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
