use actix_web::App;

use chrono::{DateTime, Duration, Timelike, Utc};
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};

use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::{
    api::{v1_app_factory, AttestationResponse},
    data_models::{asset_pair::AssetPair, event_ids::EventId, expiries::Expiry},
    schedule_context::{api_context::ApiContext, OracleContext},
    SECP,
};

use crate::test::{
    api::{get_test_server, populate_test_db, run_in_local_set},
    schedule_context::MockContext,
};

// ============================================================================
// HTTP API Tests with Mocked Pricefeed
// ============================================================================

use serde_json::json;

/// Test GET /assets endpoint
///
/// This test verifies that the assets endpoint returns the list of supported asset pairs
#[sqlx::test]
async fn test_http_get_assets(pool: PgPool) {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        let mut resp = client
            .get(srv.url("/v1/assets"))
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(resp.status(), 200, "Expected status 200");

        let assets: Vec<AssetPair> = resp.json().await.expect("Failed to parse JSON");
        assert!(!assets.is_empty(), "Expected at least one asset pair");
        assert!(
            assets.contains(&AssetPair::BtcUsd),
            "Expected BTC/USD to be in the list"
        );
    })
    .await
}

/// Test GET /asset/{asset_id}/config endpoint
///
/// This test verifies that the config endpoint returns the correct configuration
#[sqlx::test]
async fn test_http_get_config(pool: PgPool) {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        let mut resp = client
            .get(srv.url("/v1/asset/btc_usd/config"))
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(resp.status(), 200, "Expected status 200");

        let config: serde_json::Value = resp.json().await.expect("Failed to parse JSON");
        assert_eq!(config["pricefeed"], "[MOCKED]", "Expected mocked pricefeed");
        // Check for either announcementOffset or announcement_offset (camelCase vs snake_case)
        assert!(
            config.get("announcementOffset").is_some()
                || config.get("announcement_offset").is_some(),
            "Expected announcement offset"
        );
        assert!(config.get("schedule").is_some(), "Expected schedule");
    })
    .await
}

/// Test GET /oracle/publickey endpoint
///
/// This test verifies that the public key endpoint returns a valid public key
#[sqlx::test]
async fn test_http_get_public_key(pool: PgPool) {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        let mut resp = client
            .get(srv.url("/v1/oracle/publickey?assetPair=btc_usd"))
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(resp.status(), 200, "Expected status 200");

        let pubkey: serde_json::Value = resp.json().await.expect("Failed to parse JSON");
        assert!(pubkey.get("publicKey").is_some(), "Expected public key");

        // Verify the public key is a valid hex string
        let pubkey_str = pubkey["publicKey"]
            .as_str()
            .expect("Public key should be a string");
        assert_eq!(
            pubkey_str.len(),
            64,
            "Expected 32-byte public key (64 hex chars)"
        );
        // The public key might be in a different format, so let's be more flexible
        assert!(
            pubkey_str.len() >= 64,
            "Expected public key to be at least 32 bytes (64 hex chars)"
        );
    })
    .await
}

/// Test GET /asset/{asset_pair}/{event_type}/{rfc3339_time} endpoint for announcement
///
/// This test verifies that the announcement endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_get_announcement(pool: PgPool) {
    run_in_local_set(async move {
        // Create a server with pre-populated announcements
        let (context_handler, mut oracle_context, srv) = get_test_server(pool).await;
        let event_ids = populate_test_db(&mut oracle_context, 1, &context_handler).await;
        let client = awc::Client::default();

        // Extract timestamp from the first event ID
        let event_id = &event_ids[0];
        // EventId format is "btc_usd{timestamp}", so extract the timestamp part
        let event_id_str = event_id.as_ref();
        let timestamp_str = &event_id_str[7..]; // Skip "btc_usd"
        let timestamp_seconds: i64 = timestamp_str.parse().expect("Failed to parse timestamp");
        let timestamp = chrono::DateTime::from_timestamp(timestamp_seconds, 0)
            .unwrap()
            .to_rfc3339();

        let _ = context_handler.send(vec![(*event_id, Some(50000.0))]);

        let mut resp = client
            .get(srv.url(&format!("/v1/asset/btc_usd/announcement/{}", timestamp)))
            .send()
            .await
            .expect("Failed to send request");

        // Should succeed since we have announcements in the database
        assert_eq!(
            resp.status(),
            200,
            "Expected status 200 for existing announcement"
        );

        let announcements: Vec<OracleAnnouncement> =
            resp.json().await.expect("Failed to parse JSON");

        assert_eq!(announcements.len(), 1, "Expected 1 announcement");
        assert!(
            announcements[0].oracle_event.event_id == event_id.as_ref(),
            "Expected oracle event"
        );
        announcements[0].validate(&SECP).unwrap();
    })
    .await
}

/// Test GET /asset/{asset_pair}/{event_type}/{rfc3339_time} endpoint for attestation
///
/// This test verifies that the attestation endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_get_attestation(pool: PgPool) {
    run_in_local_set(async move {
        // For attestation testing, we'll use the force endpoint to create an attestation
        // and then test retrieving it
        let (context_handler, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        // First, create an attestation using the force endpoint
        let now = Utc::now();

        let forced_price = 50000.0;
        let request_body = json!({
            "maturation": now.to_rfc3339(),
            "price": forced_price
        });

        let mut force_resp = client
            .post(srv.url("/v1/force"))
            .send_json(&request_body)
            .await
            .expect("Failed to send force request");

        assert_eq!(
            force_resp.status(),
            200,
            "Expected status 200 for force attestation"
        );

        let force_response: serde_json::Value =
            force_resp.json().await.expect("Failed to parse JSON");
        let event_id = force_response["attestation"]["eventId"]
            .as_str()
            .expect("Expected event ID")
            .parse()
            .unwrap();

        context_handler
            .send(vec![(event_id, Some(forced_price))])
            .unwrap();

        // Now test getting the attestation
        let mut resp = client
            .get(srv.url(&format!(
                "/v1/asset/btc_usd/attestation/{}",
                now.to_rfc3339()
            )))
            .send()
            .await
            .expect("Failed to send request");

        // Should succeed since we created the attestation
        assert_eq!(
            resp.status(),
            200,
            "Expected status 200 for existing attestation"
        );

        let attestations: Vec<AttestationResponse> =
            resp.json().await.expect("Failed to parse JSON");

        assert_eq!(attestations.len(), 1, "Expected 1 attestation");

        assert!(attestations[0].event_id == event_id, "Expected event ID");
        assert!(
            attestations[0].signatures.len() == 30,
            "Expected signatures"
        );
        assert!(attestations[0].values.len() == 30, "Expected values");
    })
    .await
}

/// Test POST /asset/{asset_pair}/announcements/batch endpoint
///
/// This test verifies that the batch announcements endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_batch_announcements(pool: PgPool) {
    run_in_local_set(async move {
        // Create a server with pre-populated announcements
        let (context_handler, mut oracle_context, srv) = get_test_server(pool).await;
        let event_ids = populate_test_db(&mut oracle_context, 2, &context_handler).await;
        let client = awc::Client::default();

        // Extract timestamps from the event IDs
        let mut maturities = Vec::new();
        for event_id in &event_ids {
            let event_id_str = event_id.as_ref();
            let timestamp_str = &event_id_str[7..]; // Skip "btc_usd"
            let timestamp_seconds: i64 = timestamp_str.parse().expect("Failed to parse timestamp");
            let timestamp = chrono::DateTime::from_timestamp(timestamp_seconds, 0).unwrap();
            maturities.push(timestamp.to_rfc3339());
        }

        let request_body = json!({
            "maturities": maturities
        });

        let mut resp = client
            .post(srv.url("/v1/asset/btc_usd/announcements/batch"))
            .send_json(&request_body)
            .await
            .expect("Failed to send request");

        // Should succeed since we have announcements in the database
        assert_eq!(
            resp.status(),
            200,
            "Expected status 200 for batch announcements"
        );

        let announcements: Vec<OracleAnnouncement> =
            resp.json().await.expect("Failed to parse JSON");
        assert_eq!(announcements.len(), 2, "Expected 2 announcements");

        for announcement in announcements {
            announcement.validate(&SECP).unwrap();
        }
    })
    .await
}

/// Test POST /asset/{asset_pair}/{expiry}/announcements/batch endpoint
///
/// This test verifies that the batch announcements endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_batch_announcements_with_expiry(pool: PgPool) {
    run_in_local_set(async move {
        // Create a server with pre-populated announcements
        let (context_handler, oracle_context, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        let expiry = Expiry::from(Utc::now().date_naive() + Duration::days(1));

        let base_date = DateTime::from(expiry);

        let maturities = vec![
            (base_date - Duration::hours(2)),
            (base_date - Duration::hours(1)),
            base_date,
        ];

        let mut event_ids = Vec::with_capacity(maturities.len());

        for maturity_time in &maturities {
            let event_id = if maturity_time == &base_date {
                EventId::delivery_of_expiry_with_pair(AssetPair::BtcUsd, expiry)
            } else {
                EventId::forward_of_expiry_with_pair_at_timestamp(
                    AssetPair::BtcUsd,
                    expiry,
                    *maturity_time,
                )
            };

            // Set up mocked pricefeed data for all the events we're about to create
            let _ = context_handler.send(vec![(event_id, Some(50000.0 as f64))]);

            // Create announcement
            let mut announcements = oracle_context
                .oracles()
                .get(&AssetPair::BtcUsd)
                .unwrap()
                .create_announcements_at_date(*maturity_time)
                .await
                .expect("Failed to create announcement");

            event_ids.push(event_id);

            // Verify the event ID matches what we expected
            assert_eq!(announcements.pop().unwrap().oracle_event.event_id, event_id);
        }

        event_ids.iter().for_each(|&event_id| {
            context_handler
                .send(vec![(event_id, Some(50000.0 as f64))])
                .unwrap()
        });

        let request_body = json!({
            "maturities": maturities.iter().map(|m| m.to_rfc3339()).collect::<Vec<String>>()
        });

        let mut resp = client
            .post(srv.url(&format!("/v1/asset/btc_usd/{expiry}/announcements/batch")))
            .send_json(&request_body)
            .await
            .expect("Failed to send request");

        // Should succeed since we have announcements in the database
        assert_eq!(
            resp.status(),
            200,
            "Expected status 200 for existing announcement"
        );

        let announcements: Vec<OracleAnnouncement> =
            resp.json().await.expect("Failed to parse JSON");

        assert_eq!(announcements.len(), 3, "Expected 3 announcement");
    })
    .await
}

/// Test POST /force endpoint
///
/// This test verifies that the force endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_force_attestation(pool: PgPool) {
    run_in_local_set(async move {
        let (context_handler, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        // Set up mocked pricefeed data
        let now = Utc::now();
        let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, now);

        let _ = context_handler.send(vec![(event_id.clone(), Some(50000.0))]);

        let request_body = json!({
            "maturation": now.to_rfc3339(),
            "price": 50000.0
        });

        let mut resp = client
            .post(srv.url("/v1/force"))
            .send_json(&request_body)
            .await
            .expect("Failed to send request");

        assert_eq!(resp.status(), 200, "Expected status 200");

        let response: serde_json::Value = resp.json().await.expect("Failed to parse JSON");
        assert!(
            response.get("announcement").is_some(),
            "Expected announcement"
        );
        assert!(
            response.get("attestation").is_some(),
            "Expected attestation"
        );

        let attestation = &response["attestation"];
        assert!(attestation.get("eventId").is_some(), "Expected event ID");
        assert!(
            attestation.get("signatures").is_some(),
            "Expected signatures"
        );
        assert!(attestation.get("values").is_some(), "Expected values");
    })
    .await
}

/// Test error handling for invalid asset pair
///
/// This test verifies that the API returns appropriate errors for invalid asset pairs
#[sqlx::test]
async fn test_http_invalid_asset_pair(pool: PgPool) {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        // Test with an invalid asset pair
        let resp = client
            .get(srv.url("/v1/asset/invalid_pair/config"))
            .send()
            .await
            .expect("Failed to send request");

        // THIS IS WEIRD BUT ACTIX RETURNS "NOT FOUND" WHEN NOT ABLE TO DESERIALIZE THE PATH
        // https://actix.rs/docs/url-dispatch/#changing-the-default-not-found-response

        assert_eq!(
            resp.status(),
            404,
            "Expected status 404 for invalid asset pair"
        );
    })
    .await
}

/// Test error handling for invalid timestamp format
///
/// This test verifies that the API returns appropriate errors for invalid timestamp formats
#[sqlx::test]
async fn test_http_invalid_timestamp(pool: PgPool) {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        // Test with an invalid timestamp format
        let resp = client
            .get(srv.url("/v1/asset/btc_usd/announcement/invalid-timestamp"))
            .send()
            .await
            .expect("Failed to send request");

        // THIS IS WEIRD BUT ACTIX RETURNS "NOT FOUND" WHEN NOT ABLE TO DESERIALIZE THE PATH
        // https://actix.rs/docs/url-dispatch/#changing-the-default-not-found-response
        assert_eq!(
            resp.status(),
            404,
            "Expected status 404 for invalid timestamp"
        );
    })
    .await
}

/// Test error handling for future attestation request
///
/// This test verifies that the API returns appropriate errors when requesting attestation for future timestamps
#[sqlx::test]
async fn test_http_future_attestation(pool: PgPool) {
    run_in_local_set(async move {
        let (context_handler, context, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        // Set up mocked pricefeed data for future timestamp
        let future_time = Utc::now()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap()
            + Duration::hours(3);
        let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, future_time);

        let _ = context_handler.send(vec![(event_id, Some(50000.0))]);

        context
            .oracles()
            .get(&AssetPair::BtcUsd)
            .unwrap()
            .create_announcements_at_date(future_time)
            .await
            .unwrap();

        let _ = context_handler.send(vec![(event_id, Some(50000.0))]);

        let timestamp = future_time.to_rfc3339();
        let resp = client
            .get(srv.url(&format!("/v1/asset/btc_usd/attestation/{}", timestamp)))
            .send()
            .await
            .expect("Failed to send request");

        // Should get a 400 error for future attestation request
        assert_eq!(
            resp.status(),
            400,
            "Expected status 404 for future attestation"
        );
    })
    .await
}

/// Test with populated database and mocked pricefeed
///
/// This test combines database population with mocked pricefeed to test real scenarios
#[sqlx::test]
async fn test_http_with_populated_db_and_mocked_pricefeed(pool: PgPool) {
    run_in_local_set(async move {
        let channel_sender = broadcast::Sender::new(32);
        let (context_handler, oracle_context) = MockContext::new(pool).await;

        let now = Utc::now()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap()
            + Duration::minutes(5);

        let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, now);

        let api_context = ApiContext {
            oracle_context: oracle_context.clone(),
            offset_duration: chrono::Duration::minutes(2),
            channel_sender,
        };

        let srv = actix_test::start(move || {
            let factory = v1_app_factory::<MockContext>(true);

            App::new().app_data(api_context.clone()).service(factory)
        });

        let client = awc::Client::default();

        // Set up mocked pricefeed data for the same event IDs
        let _ = context_handler.send(vec![(event_id, Some(50000.0))]);

        oracle_context
            .oracles()
            .get(&AssetPair::BtcUsd)
            .unwrap()
            .create_announcements_at_date(now)
            .await
            .unwrap();

        // Set up mocked pricefeed data for the same event IDs
        context_handler
            .send(vec![(event_id, Some(50000.0))])
            .unwrap();

        // Test getting an announcement for an existing event
        let timestamp = now.to_rfc3339();
        let mut resp = client
            .get(srv.url(&format!("/v1/asset/btc_usd/announcement/{}", timestamp)))
            .send()
            .await
            .expect("Failed to send request");

        // Should succeed since we have data in the database
        assert_eq!(
            resp.status(),
            200,
            "Expected status 200 for existing announcement"
        );

        let body = resp.body().await.unwrap();
        println!("{:?}", body);

        let announcements: Vec<OracleAnnouncement> =
            serde_json::from_slice(&body).expect("Failed to parse JSON");

        assert_eq!(announcements.len(), 1, "Expected 1 announcement");
        announcements[0].validate(&SECP).unwrap();
        assert_eq!(announcements[0].oracle_event.event_id, event_id.to_string());

        context_handler
            .send(vec![(event_id, Some(50000.0))])
            .unwrap();

        let resp = client
            .get(srv.url(&format!("/v1/asset/btc_usd/attestation/{}", timestamp)))
            .send()
            .await
            .expect("Failed to send request");

        // Should succeed since we have data in the database
        assert_eq!(
            resp.status(),
            400,
            "Expected status 400 for not existing attestation"
        );

        context_handler
            .send(vec![(event_id, Some(50000.0))])
            .unwrap();

        oracle_context
            .oracles()
            .get(&AssetPair::BtcUsd)
            .unwrap()
            .attest_at_date(now)
            .await
            .unwrap();

        context_handler
            .send(vec![(event_id, Some(50000.0))])
            .unwrap();

        let mut resp = client
            .get(srv.url(&format!("/v1/asset/btc_usd/attestation/{}", timestamp)))
            .send()
            .await
            .expect("Failed to send request");

        // Should succeed since we have data in the database
        assert_eq!(
            resp.status(),
            200,
            "Expected status 200 for existing attestation"
        );

        let attestations: Vec<AttestationResponse> =
            resp.json().await.expect("Failed to parse JSON");

        assert_eq!(attestations.len(), 1, "Expected 1 attestation");

        let attestation = OracleAttestation {
            event_id: attestations[0].event_id.to_string(),
            oracle_public_key: oracle_context
                .oracles()
                .get(&AssetPair::BtcUsd)
                .unwrap()
                .get_public_key()
                .into(),
            signatures: attestations[0].signatures.clone(),
            outcomes: attestations[0]
                .values
                .iter()
                .map(|o| o.to_string())
                .collect(),
        };

        attestation.validate(&SECP, &announcements[0]).unwrap();
        assert_eq!(&*attestation.event_id, &event_id.to_string());
    })
    .await
}
