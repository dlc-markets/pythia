use std::io::{Error, ErrorKind};

use chrono::{DateTime, Utc};
use sqlx::{FromRow, Result};

use crate::{db::DBconnection, oracle::announcing::SignedEventToInsert};

impl DBconnection {
    /// Insert announcement data and meta-data in postgres DB
    pub(in crate::oracle) async fn insert_announcement(
        &self,
        event: &SignedEventToInsert,
    ) -> Result<()> {
        self.insert_many_announcements(std::slice::from_ref(event))
            .await
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
            events_sorted_by_id.is_sorted_by(
                |a, b| a.event_id_infos.as_event_id() <= b.event_id_infos.as_event_id()
            ),
            "announcements_sorted_by_id must be sorted in ascending order before inserting into database"
        );

        let estimated_nb_digits = first_event.event_descriptor.nb_digits as usize;

        let estimate_total_digits = events_sorted_by_id.len() * estimated_nb_digits;

        // We cannot collect the results directly from iteration on events to insert
        // as they must be flatten contrary to others lists.
        // To avoid as much as possible reallocations, we preallocate that we insert
        // the number of nonces of the first event times the number of event to insert plus a margin of 50%
        let mut tuple_extend = (
            Vec::with_capacity(3 * estimate_total_digits / 2),
            Vec::with_capacity(3 * estimate_total_digits / 2),
        );

        let (event_ids, digits_counts, precisions, maturities, announcement_signatures) =
            events_sorted_by_id
                .iter()
                .map(|event| {
                    // instead of collecting for nonce we have to extend:
                    tuple_extend.extend(
                        event
                            .nonces_keypairs
                            .iter()
                            .map(|kp| (kp.x_only_public_key().0.serialize(), kp.secret_bytes())),
                    );

                    Ok((
                        event.event_id_infos.as_event_id(),
                        event.event_descriptor.nb_digits as i32,
                        event.event_descriptor.precision,
                        DateTime::from_timestamp(event.maturity.into(), 0).ok_or(Error::new(
                            ErrorKind::InvalidInput,
                            "Failed to convert timestamp to DateTime",
                        ))?,
                        event.announcement_signature.serialize(),
                    ))
                })
                .collect::<Result<(Vec<_>, Vec<_>, Vec<_>, Vec<_>, Vec<_>)>>()?;

        let (nonce_publics, nonce_secrets) = tuple_extend;

        assert_eq!(
            nonce_publics.len(),
            digits_counts.iter().sum::<i32>() as usize,
            "nonce_publics length {} must match the sum of digits_counts {}: inconsistent announcement",
            nonce_publics.len(),
            digits_counts.iter().sum::<i32>()
        );

        let mut tx = self.0.begin().await?;

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
            .bind(&digits_counts)
            .bind(precisions)
            .bind(maturities)
            .bind(announcement_signatures)
            .bind(nonce_publics)
            .bind(nonce_secrets)
            .execute(&mut *tx)
            .await?;

        let affected_raws_counts = query_result.rows_affected() as usize;

        trace!("Rows affected count when inserting announcement: {affected_raws_counts}");

        let expected_affected_rows = digits_counts.into_iter().sum::<i32>() as usize;

        if affected_raws_counts != expected_affected_rows {
            if expected_affected_rows == estimate_total_digits
                && affected_raws_counts % estimated_nb_digits != 0
            {
                error!(
                    "Inserting announcements did not affect the expected number of rows: {affected_raws_counts} instead of {expected_affected_rows}"
                )
            } else {
                warn!(
                    "We tried to insert an announcement while it was already in postgres database. This is normal only if another pythia instance is running using the same database with the same private key."
                )
            }

            warn!("Announcement insertion into postgres database has been cancelled");
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }

        Ok(())
    }
}

#[derive(PartialEq, PartialOrd, FromRow)]
struct MaturityResponse {
    pub maturity: DateTime<Utc>,
}

impl DBconnection {
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
