use sqlx::{Encode, FromRow, Postgres, Result, Type};

use crate::{
    data_models::event_ids::EventId,
    db::{DBconnection, EventFromPostgres},
};
use chrono::{DateTime, Utc};

use super::ScalarsRecords;
use crate::data_models::event_ids::EventIdInfos;
use secp256k1_zkp::{Scalar, schnorr::Signature};

// Type of raw responses when querying events in DB
#[derive(Clone, Debug, FromRow)]
struct BatchedPostgresResponse {
    id: EventId,
    maturity: DateTime<Utc>,
    digits: i32,
    precision: i32,
    announcement_signature: Vec<u8>,
    nonces_public: Vec<[u8; 32]>,
    scalar_record: Vec<[u8; 32]>,
    outcome: Option<f64>,
}

macro_rules! query_with_clause {
    ($e:expr) => {
        std::concat!("
WITH e AS ( SELECT * FROM oracle.events events ", $e,  r#")
SELECT 
    e.id,
    e.maturity,
    e.digits, 
    e.precision, 
    e.announcement_signature, 
    array_agg(d.nonce_public ORDER BY d.digit_index) AS nonces_public, 
    CASE WHEN e.outcome IS NULL 
        THEN array_agg(d.nonce_secret ORDER BY d.digit_index)
        ELSE array_agg(d.signature ORDER BY d.digit_index)
        END AS scalar_record,
    e.outcome
FROM e JOIN oracle.digits d 
    ON e.id = d.event_id
GROUP BY e.id, e.digits, e.precision, e.maturity, e.announcement_signature, e.outcome
ORDER BY e.id ASC;
"#)
    };
}

impl DBconnection {
    /// Retrieve the current state of many events in oracle's DB
    pub(super) async fn get_events_with<T: GetCriteria>(
        &self,
        conditions: T,
    ) -> Result<impl ExactSizeIterator<Item = EventFromPostgres>> {
        let batch = sqlx::query_as(T::QUERY_STR)
            .bind(conditions)
            .fetch_all(&self.0)
            .await?;
        Ok(convert_batch(batch))
    }
}

pub(super) trait GetCriteria: for<'a> Encode<'a, Postgres> + Type<Postgres> {
    const QUERY_STR: &'static str;
}

impl GetCriteria for EventId {
    const QUERY_STR: &'static str = query_with_clause!("WHERE events.id = $1");
}

impl GetCriteria for &[EventId] {
    const QUERY_STR: &'static str = query_with_clause!("WHERE events.id = ANY($1::VARCHAR[])");
}

impl GetCriteria for DateTime<Utc> {
    const QUERY_STR: &'static str = query_with_clause!("WHERE events.maturity = $1");
}

// Ensure that we can convert to the rustier type with in place iteration
// const _: () = const {
//     assert!(
//         std::mem::size_of::<BatchedPostgresResponse>() == std::mem::size_of::<EventFromPostgres>()
//     );
//     assert!(
//         std::mem::align_of::<BatchedPostgresResponse>()
//             == std::mem::align_of::<EventFromPostgres>()
//     );
// };

// Convert response into more rusty type with in place iteration
fn convert_batch(
    batch: Vec<BatchedPostgresResponse>,
) -> impl ExactSizeIterator<Item = EventFromPostgres> {
    batch.into_iter().map(|r| EventFromPostgres {
        digits: r.digits as u16,
        precision: r.precision as u16,
        event_id_infos: EventIdInfos::try_from(r.id).expect("Always possible for now"),
        maturity: r.maturity,
        announcement_signature: Signature::from_slice(&r.announcement_signature[..])
            .expect("announcement_signature must have valid length inserted by pythia"),
        nonces_public: r.nonces_public.into_boxed_slice(),
        scalars_records: if let Some(outcome) = r.outcome {
            ScalarsRecords::DigitsAttestations(
                outcome,
                r.scalar_record
                    .into_iter()
                    .map(|x| Scalar::from_be_bytes(x).expect("we only insert valid scalars"))
                    .collect(),
            )
        } else {
            ScalarsRecords::DigitsSkNonce(r.scalar_record.into_boxed_slice())
        },
    })
}

#[cfg(test)]
mod split_query_check {
    use chrono::SubsecRound;
    use sqlx::PgPool;
    use std::time::Duration;

    use crate::{oracle::test::setup_oracle, pricefeeds::ImplementedPriceFeed};

    use super::*;

    // Type check the query for our DB even without where clause
    // and iteration is in place
    #[sqlx::test]
    async fn test_split_big_query_check(tbd: PgPool) {
        let oracle = setup_oracle(12, 32, ImplementedPriceFeed::Lnmarkets);
        let now = Utc::now().round_subsecs(0);
        let dates = [60, 3600, 24 * 3600, 7 * 24 * 3600]
            .iter()
            .map(|t| now - Duration::new(*t, 0))
            .collect::<Vec<_>>();
        let db = DBconnection(tbd);

        for date in &dates {
            oracle
                .create_announcements_at_date(&db, *date)
                .await
                .unwrap()
                .pop()
                .unwrap();
        }
        // Query the database without where clause
        let batch = sqlx::query_as(query_with_clause!(""))
            .fetch_all(&db.0)
            .await
            .unwrap();

        // Unrestricted query all events in db
        assert_eq!(batch.len(), dates.len());

        let alloc_ptr = batch.as_ptr();
        let converted = convert_batch(batch).collect::<Vec<_>>();

        // Check pointer stays the same after conversion
        assert_eq!(alloc_ptr, converted.as_ptr().cast());

        let batch2: Vec<BatchedPostgresResponse> =
            sqlx::query_as(query_with_clause!("WHERE events.maturity != $1"))
                .bind(dates[0])
                .fetch_all(&db.0)
                .await
                .unwrap();

        // Check the where clause only removes one event
        assert_eq!(batch2.len(), dates.len() - 1);
    }
}
