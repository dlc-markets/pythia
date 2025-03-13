use chrono::{DateTime, Utc};
use dlc_messages::oracle_msgs::{EventDescriptor, OracleAnnouncement, OracleAttestation};
use secp256k1_zkp::{schnorr::Signature, Scalar, XOnlyPublicKey};
use sqlx::{
    postgres::{PgConnectOptions, PgPool, PgPoolOptions},
    Result,
};

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

#[derive(Clone)]
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
    pub maturity: Option<DateTime<Utc>>,
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
            sk_nonces.as_slice(),
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

    pub(super) async fn insert_many_announcements(
        &self,
        announcements_with_sk_nonces: &[OracleAnnouncementWithSkNonces],
    ) -> Result<()> {
        if announcements_with_sk_nonces.is_empty() {
            return Ok(());
        }

        // Prepare arrays for bulk insertion
        let mut event_ids: Vec<String> = Vec::new();
        let mut digits_counts: Vec<i32> = Vec::new();
        let mut precisions: Vec<i32> = Vec::new();
        let mut maturities: Vec<DateTime<Utc>> = Vec::new();
        let mut announcement_signatures: Vec<Vec<u8>> = Vec::new();

        // For digits table - we'll collect all items for a single UNNEST operation
        let mut digit_event_ids = Vec::new();
        let mut digit_indexes = Vec::new();
        let mut nonce_publics = Vec::new();
        let mut nonce_secrets = Vec::new();

        for (announcement, outstanding_sk_nonces) in announcements_with_sk_nonces {
            let EventDescriptor::DigitDecompositionEvent(ref digit) =
                announcement.oracle_event.event_descriptor
            else {
                return Err(sqlx::Error::TypeNotFound {
                    type_name: "Only DigitDecomposition event type is supported".to_string(),
                });
            };

            event_ids.push(announcement.oracle_event.event_id.clone());
            digits_counts.push(digit.nb_digits as i32);
            precisions.push(digit.precision);
            maturities.push(
                DateTime::from_timestamp(announcement.oracle_event.event_maturity_epoch.into(), 0)
                    .unwrap(),
            );
            announcement_signatures.push(announcement.announcement_signature.as_ref().to_vec());

            digit_event_ids.push(vec![
                announcement.oracle_event.event_id.to_owned();
                digit.nb_digits as usize
            ]);

            digit_indexes.push((0..digit.nb_digits as i32).collect::<Vec<i32>>());

            nonce_publics.push(
                announcement
                    .oracle_event
                    .oracle_nonces
                    .iter()
                    .map(|x| x.serialize().to_vec())
                    .collect::<Vec<Vec<u8>>>(),
            );
            let sk_nonces = outstanding_sk_nonces
                .iter()
                .map(|x| x.into())
                .collect::<Vec<Vec<u8>>>();
            nonce_secrets.push(sk_nonces);
        }

        // flatten the arrays
        let digit_event_ids: Vec<String> = digit_event_ids.into_iter().flatten().collect();
        let digit_indexes: Vec<i32> = digit_indexes.into_iter().flatten().collect();
        let nonce_publics: Vec<Vec<u8>> = nonce_publics.into_iter().flatten().collect();
        let nonce_secrets: Vec<Vec<u8>> = nonce_secrets.into_iter().flatten().collect();

        let query_result = sqlx::query!(
            "WITH events AS (
                INSERT INTO oracle.events (id, digits, precision, maturity, announcement_signature) 
                SELECT * FROM UNNEST($1::VARCHAR[], $2::INT[], $3::INT[], $4::TIMESTAMPTZ[], $5::BYTEA[])
                ON CONFLICT DO NOTHING
                RETURNING id
            )
            INSERT INTO oracle.digits (event_id, digit_index, nonce_public, nonce_secret)
            SELECT * FROM UNNEST($6::VARCHAR[], $7::INT[], $8::BYTEA[], $9::BYTEA[])
            ON CONFLICT DO NOTHING
            ",
            &event_ids,
            &digits_counts,
            &precisions,
            &maturities,
            &announcement_signatures,
            &digit_event_ids,
            &digit_indexes,
            &nonce_publics,
            &nonce_secrets,
        ).execute(&self.0).await?;

        let affected_raws_counts = query_result.rows_affected();

        trace!(
            "Rows affected count when inserting announcement: {}",
            affected_raws_counts
        );
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
                            .unwrap(),
                            nonce_public: nonces_public
                                .into_iter()
                                .map(|ref s| XOnlyPublicKey::from_slice(s).unwrap())
                                .collect(),
                            scalars_records: ScalarsRecords::DigitsSkNonce(Vec::new()),
                        }
                    },
                )
                .collect(),
        ))
    }

    pub(super) async fn get_non_existing_maturity(
        &self,
        maturities: &[DateTime<Utc>],
    ) -> Result<Vec<DateTime<Utc>>> {
        let maturity_response = sqlx::query_as!(
            MaturityResponse,
            "
            WITH maturity_array AS (
                SELECT UNNEST($1::TIMESTAMPTZ[]) AS maturity
            )
            SELECT
                maturity
            FROM
                maturity_array
            WHERE maturity NOT IN (SELECT maturity FROM oracle.events);",
            &maturities
        )
        .fetch_all(&self.0)
        .await?;
        let maturity_array = maturity_response
            .into_iter()
            .filter_map(|x| x.maturity)
            .collect();
        Ok(maturity_array)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{oracle::Oracle, pricefeeds::ImplementedPriceFeed, AssetPair, AssetPairInfo, SECP};
    use chrono::Duration;
    use dlc_messages::oracle_msgs::DigitDecompositionEventDescriptor;
    use secp256k1_zkp::{Keypair, SecretKey};
    use sqlx::PgPool;
    use std::str::FromStr;
    type Error = Box<dyn std::error::Error>;
    type Result<T> = std::result::Result<T, Error>;

    fn create_test_oracle(db: &DBconnection) -> Result<Oracle> {
        // Create a test oracle
        let asset_pair_info = AssetPairInfo {
            pricefeed: ImplementedPriceFeed::Lnmarkets,
            asset_pair: AssetPair::default(),
            event_descriptor: DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: false,
                unit: "usd".to_owned(),
                precision: 0,
                nb_digits: 20,
            },
        };
        let secret_key = SecretKey::from_str(
            "d0a26c65de0b4b853432c3931ee280f67b9c52de33e1b3aecb04edc1ec40ef4a",
        )?;
        let keypair = Keypair::from_secret_key(&SECP, &secret_key);
        let oracle = Oracle::new(asset_pair_info.clone(), db.clone(), keypair);
        Ok(oracle)
    }

    fn create_test_oracle_with_digits(db: &DBconnection, nb_digit: u16) -> Result<Oracle> {
        // Create a test oracle
        let asset_pair_info = AssetPairInfo {
            pricefeed: ImplementedPriceFeed::Lnmarkets,
            asset_pair: AssetPair::default(),
            event_descriptor: DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: false,
                unit: "usd".to_owned(),
                precision: 0,
                nb_digits: nb_digit,
            },
        };
        let secret_key = SecretKey::from_str(
            "d0a26c65de0b4b853432c3931ee280f67b9c52de33e1b3aecb04edc1ec40ef4a",
        )?;
        let keypair = Keypair::from_secret_key(&SECP, &secret_key);
        let oracle = Oracle::new(asset_pair_info.clone(), db.clone(), keypair);
        Ok(oracle)
    }
    mod test_get_non_existing_maturity {
        use super::*;
        async fn insert_test_events(db: &DBconnection, maturities: &[DateTime<Utc>]) {
            // Create a dummy signature for announcements
            let dummy_sig = Signature::from_slice(&[0u8; 64]).unwrap();

            for (i, &maturity) in maturities.iter().enumerate() {
                sqlx::query!(
                "INSERT INTO oracle.events (id, digits, precision, maturity, announcement_signature) 
                 VALUES ($1, $2, $3, $4, $5)",
                format!("test_event_{}", i),
                3_i32,
                0_i32,
                maturity,
                dummy_sig.as_ref().to_vec(),
            )
            .execute(&db.0)
            .await
            .unwrap();
            }
        }

        #[sqlx::test]
        async fn test_get_non_existing_maturity(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);

            // Create some test maturity dates
            let now = Utc::now();
            let existing_maturities = [now, now + Duration::hours(1), now + Duration::hours(2)];

            // Insert events with these maturity dates
            insert_test_events(&db, &existing_maturities).await;

            // Create a mix of existing and non-existing maturity dates to test
            let non_existing_maturities = [now + Duration::hours(3), now + Duration::hours(4)];

            let test_maturities = vec![
                existing_maturities[0],     // Exists
                non_existing_maturities[0], // Doesn't exist
                existing_maturities[1],     // Exists
                non_existing_maturities[1], // Doesn't exist
                existing_maturities[2],     // Exists
            ];

            // Call the function
            let result = db.get_non_existing_maturity(&test_maturities).await?;

            // Verify the result contains only the non-existing maturities
            assert_eq!(result.len(), 2);
            assert!(
                result.contains(&non_existing_maturities[0]),
                "non_existing_maturities: {:?} should contains {:?}",
                result,
                non_existing_maturities[0]
            );
            assert!(
                result.contains(&non_existing_maturities[1]),
                "non_existing_maturities: {:?} should contains {:?}",
                result,
                non_existing_maturities[1]
            );

            // Verify the result doesn't contain any existing maturities
            assert!(
                !result.contains(&existing_maturities[0]),
                "non_existing_maturities: {:?} should not contains {:?}",
                result,
                existing_maturities[0]
            );
            assert!(
                !result.contains(&existing_maturities[1]),
                "non_existing_maturities: {:?} should not contains {:?}",
                result,
                existing_maturities[1]
            );
            assert!(
                !result.contains(&existing_maturities[2]),
                "non_existing_maturities: {:?} should not contains {:?}",
                result,
                existing_maturities[2]
            );

            Ok(())
        }

        #[sqlx::test]
        async fn test_get_non_existing_maturity_empty_input(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);
            let result = db.get_non_existing_maturity(&[]).await?;
            assert!(
                result.is_empty(),
                "non_existing_maturities: {:?} should be empty",
                result
            );
            Ok(())
        }

        #[sqlx::test]
        async fn test_get_non_existing_maturity_all_maturity_exist(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);
            // Create and insert test maturity dates
            let now = Utc::now();
            let existing_maturities = [now, now + Duration::hours(1)];
            insert_test_events(&db, &existing_maturities).await;
            // Test with only existing maturity dates
            let result = db.get_non_existing_maturity(&existing_maturities).await?;
            assert!(
                result.is_empty(),
                "non_existing_maturities: {:?} should be empty",
                result
            );
            Ok(())
        }

        #[sqlx::test]
        async fn test_get_non_existing_maturity_none_exist(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);
            // Create test dates that don't exist in DB
            let now = Utc::now();
            let non_existing_maturities = [now, now + Duration::hours(1)];

            // Test with only non-existing maturity dates (insert_test_events is not called)
            let result = db
                .get_non_existing_maturity(&non_existing_maturities)
                .await?;

            // All should be returned
            assert_eq!(
                result.len(),
                non_existing_maturities.len(),
                "non_existing_maturities: {:?} should contains {:?} elements",
                result,
                non_existing_maturities.len()
            );
            for maturity in &non_existing_maturities {
                assert!(
                    result.contains(maturity),
                    "non_existing_maturities: {:?} should contains {:?}",
                    result,
                    maturity
                );
            }
            Ok(())
        }
    }

    mod test_insert_many_announcements {
        use super::*;

        #[sqlx::test]
        async fn test_insert_many_announcements_empty(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);

            // Test with empty input
            let result = db.insert_many_announcements(&[]).await;

            // Should succeed with no error
            assert!(result.is_ok());

            Ok(())
        }

        #[sqlx::test]
        async fn test_insert_many_announcements_single(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);

            // Create a test oracle
            let oracle = create_test_oracle(&db)?;

            // Create a test announcement from a secret_key
            let now = Utc::now();
            let announcement_with_sk_nonces = oracle.prepare_announcement(now)?;

            // Insert the announcement
            db.insert_many_announcements(&[announcement_with_sk_nonces.clone()])
                .await?;

            // Verify the announcement was inserted
            let event = db
                .get_event(&announcement_with_sk_nonces.0.oracle_event.event_id)
                .await?;
            assert!(event.is_some());
            let event = event.expect("event should exist");

            // Check event properties
            assert_eq!(event.digits, 20);
            assert_eq!(event.nonce_public.len(), 20);

            // Check if we can retrieve the secret nonces
            match event.scalars_records {
                ScalarsRecords::DigitsSkNonce(secret_nonces) => {
                    assert_eq!(secret_nonces.len(), 20);
                    for (i, nonce) in secret_nonces.iter().enumerate() {
                        assert_eq!(nonce, &secret_nonces[i]);
                    }
                }
                _ => panic!("Expected DigitsSkNonce"),
            }

            Ok(())
        }

        #[sqlx::test]
        async fn test_insert_many_announcements_multiple(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);

            // Create some test oracles
            let oracle20 = create_test_oracle_with_digits(&db, 20)?;
            let oracle8 = create_test_oracle_with_digits(&db, 8)?;

            // Create multiple test announcements
            let now = Utc::now();
            let maturations_even = [now, now + Duration::hours(2), now + Duration::hours(4)];
            let maturations_odd = [
                now + Duration::hours(1),
                now + Duration::hours(3),
                now + Duration::hours(5),
            ];
            let announcements_with_sk_nonces20 =
                oracle20.prepare_announcements(&maturations_even)?;
            let announcements_with_sk_nonces8 = oracle8.prepare_announcements(&maturations_odd)?;

            // Insert the announcements
            db.insert_many_announcements(&announcements_with_sk_nonces20)
                .await?;
            db.insert_many_announcements(&announcements_with_sk_nonces8)
                .await?;

            let event1 = db
                .get_event(&announcements_with_sk_nonces20[0].0.oracle_event.event_id)
                .await?
                .expect("event should exist");
            assert_eq!(event1.digits, 20);
            assert_eq!(event1.nonce_public.len(), 20);

            let event2 = db
                .get_event(&announcements_with_sk_nonces8[0].0.oracle_event.event_id)
                .await?
                .expect("event should exist");
            assert_eq!(event2.digits, 8);
            assert_eq!(event2.nonce_public.len(), 8);

            Ok(())
        }

        #[sqlx::test]
        async fn test_insert_many_announcements_idempotent(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);

            // Create a test oracle
            let oracle = create_test_oracle(&db)?;

            // Create a test announcement from a secret_key
            let now = Utc::now();
            let announcement_with_sk_nonces = oracle.prepare_announcement(now)?;

            // Insert the announcement twice
            db.insert_many_announcements(&[announcement_with_sk_nonces.clone()])
                .await?;
            db.insert_many_announcements(&[announcement_with_sk_nonces.clone()])
                .await?;

            // Verify the announcement was inserted only once
            let events = db
                .get_many_events([announcement_with_sk_nonces.0.oracle_event.event_id].to_vec())
                .await?
                .unwrap_or_default();
            assert_eq!(events.len(), 1);

            Ok(())
        }
    }
}
