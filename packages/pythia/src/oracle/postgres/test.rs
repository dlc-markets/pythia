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
    let secret_key =
        SecretKey::from_str("d0a26c65de0b4b853432c3931ee280f67b9c52de33e1b3aecb04edc1ec40ef4a")?;
    let keypair = Keypair::from_secret_key(&SECP, &secret_key);
    let oracle = Oracle::new(asset_pair_info, db.clone(), keypair);
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
    let secret_key =
        SecretKey::from_str("d0a26c65de0b4b853432c3931ee280f67b9c52de33e1b3aecb04edc1ec40ef4a")?;
    let keypair = Keypair::from_secret_key(&SECP, &secret_key);
    let oracle = Oracle::new(asset_pair_info.clone(), db.clone(), keypair);
    Ok(oracle)
}

mod test_get_non_existing_sorted_maturity {
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
    async fn test_get_non_existing_sorted_maturity(pool: PgPool) -> Result<()> {
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
        let result = db
            .get_non_existing_sorted_maturity(&test_maturities)
            .await?;

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
    async fn test_get_non_existing_sorted_maturity_empty_input(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);
        let result = db.get_non_existing_sorted_maturity(&[]).await?;
        assert!(
            result.is_empty(),
            "non_existing_maturities: {:?} should be empty",
            result
        );
        Ok(())
    }

    #[sqlx::test]
    async fn test_get_non_existing_sorted_maturity_all_maturity_exist(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);
        // Create and insert test maturity dates
        let now = Utc::now();
        let existing_maturities = [now, now + Duration::hours(1)];
        insert_test_events(&db, &existing_maturities).await;
        // Test with only existing maturity dates
        let result = db
            .get_non_existing_sorted_maturity(&existing_maturities)
            .await?;
        assert!(
            result.is_empty(),
            "non_existing_maturities: {:?} should be empty",
            result
        );
        Ok(())
    }

    #[sqlx::test]
    async fn test_get_non_existing_sorted_maturity_none_exist(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);
        // Create test dates that don't exist in DB
        let now = Utc::now();
        let non_existing_maturities = [now, now + Duration::hours(1)];

        // Test with only non-existing maturity dates (insert_test_events is not called)
        let result = db
            .get_non_existing_sorted_maturity(&non_existing_maturities)
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
    use secp256k1_zkp::rand::thread_rng;

    use super::*;

    async fn get_number_of_rows(db: &DBconnection, table: &str) -> Result<i64> {
        let query = format!("SELECT COUNT(*) FROM oracle.{}", table);
        let rows_affected = sqlx::query_scalar::<_, i64>(&query)
            .fetch_one(&db.0)
            .await?;
        Ok(rows_affected)
    }

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
        let announcement_with_sk_nonces = oracle.prepare_announcement(now, &mut thread_rng())?;

        // Insert the announcement
        db.insert_many_announcements(core::slice::from_ref(&announcement_with_sk_nonces))
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

        // Verify public nonces match what we generated
        for (i, nonce) in event.nonce_public.iter().enumerate() {
            assert_eq!(
                nonce, &announcement_with_sk_nonces.0.oracle_event.oracle_nonces[i],
                "Public nonce at index {} for event id {} doesn't match original announcement",
                i, announcement_with_sk_nonces.0.oracle_event.event_id
            );
        }
        // Check if we can retrieve the secret nonces
        match event.scalars_records {
            ScalarsRecords::DigitsSkNonce(secret_nonces) => {
                assert_eq!(secret_nonces.len(), 20);
                for (i, nonce) in secret_nonces.iter().enumerate() {
                    assert_eq!(
                        nonce, &announcement_with_sk_nonces.1[i],
                        "Secret nonce at index {} for event id {} doesn't match original announcement",
                        i, announcement_with_sk_nonces.0.oracle_event.event_id
                    );
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
        let maturations_20 = [now, now + Duration::hours(2), now + Duration::hours(4)];
        let maturations_8 = [
            now + Duration::hours(1),
            now + Duration::hours(3),
            now + Duration::hours(5),
        ];
        let announcements_with_sk_nonces20 = oracle20.prepare_announcements(&maturations_20)?;
        let announcements_with_sk_nonces8 = oracle8.prepare_announcements(&maturations_8)?;

        // Get number of rows before inserting announcements (oracle with 20 digits)
        let mut rows_at_start =
            get_number_of_rows(&db, "digits").await? + get_number_of_rows(&db, "events").await?;

        // Insert the announcements (oracle with 20 digits)
        db.insert_many_announcements(&announcements_with_sk_nonces20)
            .await?;

        // Check number of affected rows
        let rows_20_after_insert =
            get_number_of_rows(&db, "digits").await? + get_number_of_rows(&db, "events").await?;
        let affected_rows_20 = rows_20_after_insert - rows_at_start;
        let expected_rows_20 = (maturations_20.len() as i64) * (1 + 20);
        assert_eq!(affected_rows_20, expected_rows_20);

        // Get number of rows before inserting announcements (oracle wth 8 digits)
        rows_at_start = rows_20_after_insert;
        db.insert_many_announcements(&announcements_with_sk_nonces8)
            .await?;

        // Check number of affected rows
        let rows_8_after_insert =
            get_number_of_rows(&db, "digits").await? + get_number_of_rows(&db, "events").await?;
        let affected_rows_8 = rows_8_after_insert - rows_at_start;
        let expected_rows_8 = (maturations_8.len() as i64) * (1 + 8);
        assert_eq!(affected_rows_8, expected_rows_8);

        // Check events
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
        let announcement_with_sk_nonces = oracle.prepare_announcement(now, &mut thread_rng())?;

        // Insert the announcement twice
        db.insert_many_announcements(core::slice::from_ref(&announcement_with_sk_nonces))
            .await?;
        db.insert_many_announcements(core::slice::from_ref(&announcement_with_sk_nonces))
            .await?;

        // Verify the announcement was inserted only once
        let events = db
            .get_many_events([announcement_with_sk_nonces.0.oracle_event.event_id].to_vec())
            .await?
            .unwrap_or_default();
        assert_eq!(events.len(), 1);

        Ok(())
    }

    #[sqlx::test]
    async fn test_insert_many_announcements_maximum_query_parameter(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Create a test oracle with minimal digits to allow more announcements
        let nb_digits = 2; // Using fewer digits to create more announcements
        let oracle = create_test_oracle_with_digits(&db, nb_digits)?;

        // Create many announcements to potentially hit the parameter limit
        // PostgreSQL has a default limit of 65535 parameters per query
        // Each announcement uses (1 event_id + 1 digits + 1 precision + 1 maturity + 1 sig + nb_digits*2 for nonces) parameters
        // So parameter count = num_announcements * (5 + nb_digits*2)
        // Let's create 7282 announcements with 2 digits each
        // This would be 7282 * (5 + 2*2) = 65538 parameters (> 655535)
        // Since we use UNNEST in our query, this test should pass
        const NUM_ANNOUNCEMENTS: usize = 7282;

        // Generate maturity dates
        let now = Utc::now();
        let mut maturities = Vec::with_capacity(NUM_ANNOUNCEMENTS);
        for i in 0..NUM_ANNOUNCEMENTS {
            maturities.push(now + Duration::minutes(i as i64));
        }

        // Prepare all announcements
        let announcements_with_sk_nonces = oracle.prepare_announcements(&maturities)?;

        // Count rows before insertion to calculate affected rows later
        let rows_before =
            get_number_of_rows(&db, "digits").await? + get_number_of_rows(&db, "events").await?;

        // Insert all announcements at once
        db.insert_many_announcements(&announcements_with_sk_nonces)
            .await?;

        // Count rows after insertion
        let rows_after =
            get_number_of_rows(&db, "digits").await? + get_number_of_rows(&db, "events").await?;

        // Calculate expected rows:
        // Each event adds 1 row to events table and nb_digits rows to digits table
        let expected_affected_rows = (NUM_ANNOUNCEMENTS as i64) * (1 + nb_digits as i64);
        let actual_affected_rows = rows_after - rows_before;

        // Verify number of affected rows matches expected
        assert_eq!(
            actual_affected_rows, expected_affected_rows,
            "Expected {} rows to be affected, but got {}",
            expected_affected_rows, actual_affected_rows
        );

        // Verify we can retrieve all announcements
        let sample_indexes = [0, NUM_ANNOUNCEMENTS / 2, NUM_ANNOUNCEMENTS - 1];
        for &idx in &sample_indexes {
            let event_id = &announcements_with_sk_nonces[idx].0.oracle_event.event_id;
            let event = db.get_event(event_id).await?.expect("event should exist");

            // Check event properties
            assert_eq!(event.digits, nb_digits);
            assert_eq!(event.nonce_public.len(), nb_digits as usize);
            println!(
                "announcement: {:#?}, event: {event:#?}",
                announcements_with_sk_nonces[idx]
            );

            // Verify public nonces match what we generated
            for (i, nonce) in event.nonce_public.iter().enumerate() {
                assert_eq!(
                    nonce,
                    &announcements_with_sk_nonces[idx]
                        .0
                        .oracle_event
                        .oracle_nonces[i],
                    "Public nonce at index {} for event id {} doesn't match original announcement",
                    i,
                    idx
                );
            }

            // Verify secret nonces match what we generated
            match &event.scalars_records {
                ScalarsRecords::DigitsSkNonce(secret_nonces) => {
                    assert_eq!(
                        secret_nonces.len(),
                        nb_digits as usize,
                        "Expected {} secret nonces, got {}",
                        nb_digits,
                        secret_nonces.len()
                    );

                    for (i, nonce) in secret_nonces.iter().enumerate() {
                        assert_eq!(
                            nonce,
                            &announcements_with_sk_nonces[idx].1[i],
                            "Secret nonce at index {} for event id {} doesn't match original announcement",
                            i, idx
                        );
                    }
                }
                _ => panic!("Expected DigitsSkNonce for event that hasn't been attested"),
            }
        }

        // Check batch retrieval works with a subset of events
        let batch_event_ids: Vec<String> = sample_indexes
            .iter()
            .map(|&idx| {
                announcements_with_sk_nonces[idx]
                    .0
                    .oracle_event
                    .event_id
                    .clone()
            })
            .collect();

        let batch_events = db
            .get_many_events(batch_event_ids)
            .await?
            .expect("batch events should exist");

        assert_eq!(batch_events.len(), sample_indexes.len());

        Ok(())
    }

    #[sqlx::test]
    #[should_panic]
    async fn test_insert_many_announcements_wrong_insertion_order(pool: PgPool) {
        // Create a DB connection
        let db = DBconnection(pool);
        let nb_digits = 20;

        // Create test oracles with different digits to make order verification clearer
        let oracle_10 =
            create_test_oracle_with_digits(&db, nb_digits).expect("test oracle should be created");

        // Create announcements with different maturity times and digit counts
        let now = Utc::now();
        let announcements_with_sk_nonces = vec![
            oracle_10
                .prepare_announcement(now + Duration::minutes(3), &mut thread_rng())
                .expect("test announcement 1 should be created"),
            oracle_10
                .prepare_announcement(now, &mut thread_rng())
                .expect("test announcement 2 should be created"),
            oracle_10
                .prepare_announcement(now + Duration::minutes(1), &mut thread_rng())
                .expect("test announcement 3 should be created"),
        ];

        // Insert all announcements at once, this should panic
        db.insert_many_announcements(&announcements_with_sk_nonces)
            .await
            .unwrap();
    }
}

mod test_get_many_events {
    use super::*;
    use secp256k1_zkp::rand::thread_rng;

    #[sqlx::test]
    async fn test_get_many_events_empty(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Test with empty input
        let result = db.get_many_events(vec![]).await?;

        // Should return Some with empty vector
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 0);

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_many_events_none_exist(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Test with non-existent events
        let non_existent_ids = vec![
            "non_existent_event_1".to_string(),
            "non_existent_event_2".to_string(),
        ];
        let result = db.get_many_events(non_existent_ids).await?;

        // Should return None since the events don't exist
        assert!(result.is_none());

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_many_events_single(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Create a test oracle
        let oracle = create_test_oracle(&db)?;

        // Create a test announcement
        let now = Utc::now();
        let announcement_with_sk_nonces = oracle.prepare_announcement(now, &mut thread_rng())?;

        // Insert the announcement
        db.insert_many_announcements(core::slice::from_ref(&announcement_with_sk_nonces))
            .await?;

        // Get the event ID
        let event_id = announcement_with_sk_nonces.0.oracle_event.event_id;

        // Test get_many_events with a single event ID
        let result = db.get_many_events(vec![event_id]).await?;

        // Should return Some with one event
        assert!(result.is_some());
        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        // Verify event properties
        let event = &events[0];
        assert_eq!(event.digits, 20);
        assert_eq!(event.nonce_public.len(), 20);

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_many_events_multiple(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Create test oracles with different digits
        let oracle20 = create_test_oracle_with_digits(&db, 20)?;
        let oracle8 = create_test_oracle_with_digits(&db, 8)?;

        // Create announcements with different maturity times
        let now = Utc::now();
        let maturity1 = now + Duration::hours(1);
        let maturity2 = now + Duration::hours(2);

        let announcement1 = oracle20.prepare_announcement(maturity1, &mut thread_rng())?;
        let announcement2 = oracle8.prepare_announcement(maturity2, &mut thread_rng())?;

        // Insert the announcements
        db.insert_many_announcements(&[announcement1.clone(), announcement2.clone()])
            .await?;

        // Get event IDs
        let event_id1 = announcement1.0.oracle_event.event_id;
        let event_id2 = announcement2.0.oracle_event.event_id;

        // Test get_many_events with both event IDs
        let result = db.get_many_events(vec![event_id1, event_id2]).await?;

        // Should return Some with two events
        assert!(result.is_some());
        let events = result.unwrap();
        assert_eq!(events.len(), 2);

        // Since events are ordered by ID, we need to find which is which by digits count
        let event20 = events
            .iter()
            .find(|e| e.digits == 20)
            .expect("20-digit event not found");
        let event8 = events
            .iter()
            .find(|e| e.digits == 8)
            .expect("8-digit event not found");

        // Verify event properties
        assert_eq!(event20.nonce_public.len(), 20);
        assert_eq!(event8.nonce_public.len(), 8);

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_many_events_duplicate_ids(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Create a test oracle
        let oracle = create_test_oracle(&db)?;

        // Create a test announcement
        let now = Utc::now();
        let announcement_with_sk_nonces = oracle.prepare_announcement(now, &mut thread_rng())?;

        // Insert the announcement
        db.insert_many_announcements(core::slice::from_ref(&announcement_with_sk_nonces))
            .await?;

        // Get the event ID
        let event_id = announcement_with_sk_nonces.0.oracle_event.event_id;

        // Test get_many_events with duplicated event IDs
        let result = db
            .get_many_events(vec![event_id.clone(), event_id.clone(), event_id])
            .await?;

        // Should return Some with one event (duplicates should be removed)
        assert!(result.is_some());
        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_many_events_mixed_existence(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Create a test oracle
        let oracle = create_test_oracle(&db)?;

        // Create a test announcement
        let now = Utc::now();
        let announcement_with_sk_nonces = oracle.prepare_announcement(now, &mut thread_rng())?;

        // Insert the announcement
        db.insert_many_announcements(core::slice::from_ref(&announcement_with_sk_nonces))
            .await?;

        // Get the event ID
        let event_id = announcement_with_sk_nonces.0.oracle_event.event_id;

        // Test get_many_events with a mix of existing and non-existing event IDs
        let result = db
            .get_many_events(vec![event_id, "non_existent_event".to_string()])
            .await?;

        // Should return None since not all events exist
        assert!(result.is_none());

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_many_events_after_attestation(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Create a test oracle
        let oracle = create_test_oracle(&db)?;

        // Create a test announcement
        let now = Utc::now();
        let announcement_with_sk_nonces = oracle.prepare_announcement(now, &mut thread_rng())?;
        let event_id = announcement_with_sk_nonces.0.oracle_event.event_id.clone();

        // Insert the announcement
        db.insert_many_announcements(core::slice::from_ref(&announcement_with_sk_nonces))
            .await?;

        // Create attestation and update to attestation
        let outcome = 42.0;
        let attestation = oracle
            .try_attest_event(&event_id)
            .await?
            .expect("Should be able to attest");
        db.update_to_attestation(&event_id, &attestation, outcome)
            .await?;

        // Test get_many_events after attestation
        let result = db.get_many_events(vec![event_id]).await?;

        // Should return Some with one event
        assert!(result.is_some());
        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        // Verify event properties
        let event = &events[0];
        assert_eq!(event.digits, 20);

        // When using get_many_events, the scalars_records are initialized as DigitsSkNonce with empty vector
        // even though the event has been attested, because this function doesn't fetch outcome/signatures
        match &event.scalars_records {
            ScalarsRecords::DigitsSkNonce(secret_nonces) => {
                assert!(secret_nonces.is_empty());
            }
            _ => panic!("Expected DigitsSkNonce with empty vector"),
        }

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_many_events_large_batch(pool: PgPool) -> Result<()> {
        // Create a DB connection
        let db = DBconnection(pool);

        // Create a test oracle with few digits to reduce memory usage
        let oracle = create_test_oracle_with_digits(&db, 1)?;

        // Create a moderate batch of announcements (50 should be enough to test batch processing)
        const BATCH_SIZE: usize = 50;
        let now = Utc::now();

        let mut announcements = Vec::with_capacity(BATCH_SIZE);
        let mut event_ids = Vec::with_capacity(BATCH_SIZE);

        for i in 0..BATCH_SIZE {
            let maturity = now + Duration::minutes(i as i64);
            let announcement = oracle.prepare_announcement(maturity, &mut thread_rng())?;
            event_ids.push(announcement.0.oracle_event.event_id.clone());
            announcements.push(announcement);
        }

        // Insert all announcements
        db.insert_many_announcements(&announcements).await?;

        // Test get_many_events with all event IDs
        let result = db.get_many_events(event_ids).await?;

        // Should return Some with BATCH_SIZE events
        assert!(result.is_some());
        let events = result.unwrap();
        assert_eq!(events.len(), BATCH_SIZE);

        Ok(())
    }
}
