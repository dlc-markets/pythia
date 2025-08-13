use std::future;

use actix_codec::Framed;
use actix_web::App;
use actix_ws::Message;
use awc::{
    ws::{Codec, Frame},
    BoxedSocket, Client,
};
use chrono::{Duration, Timelike, Utc};
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use futures_util::{SinkExt, StreamExt};
use json_rpc_types::{Id, Request, Version};
use sqlx::PgPool;
use tokio::{
    sync::broadcast,
    task::{JoinError, LocalSet},
};

use crate::{
    api::{v1_app_factory, AttestationResponse},
    data_models::{asset_pair::AssetPair, event_ids::EventId},
    schedule_context::{
        api_context::ApiContext,
        test::{HandlerToMock, MockContext},
        OracleContext,
    },
    SECP,
};

use super::{error::PythiaApiError, ws::RequestContent, EventChannel, EventType, GetRequest};

type JoinResult = Result<(), JoinError>;

async fn run_in_local_set<F>(f: F) -> JoinResult
where
    F: std::future::Future<Output = ()> + 'static,
{
    let local = LocalSet::new();
    let handle = local.spawn_local(f);
    local.await;
    handle.await
}

/// Populates the database with announcement events for testing
///
/// Creates a specified number of announcements with different maturity times
/// and stores their event IDs for later use in tests.
/// Sets up mocked pricefeed data before creating announcements.
pub async fn populate_test_db(
    mock_context: &mut MockContext,
    count: usize,
    context_handler: &HandlerToMock,
) -> Vec<EventId> {
    let now = Utc::now()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();

    // Pre-generate event IDs and set up mocked pricefeed data
    let mut event_ids = Vec::with_capacity(count);

    // Find the BTC/USD oracle
    let oracle = mock_context
        .oracles()
        .get(&AssetPair::BtcUsd)
        .expect("BTC/USD oracle must exist");

    // Create announcements at different times
    for i in 0..count {
        let maturity_time = now + Duration::hours(i as i64 + 1);

        let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, maturity_time);
        // Set up mocked pricefeed data for all the events we're about to create
        let _ = context_handler.send(vec![(event_id, Some(50000.0 + i as f64))]);

        // Create announcement
        let mut announcements = oracle
            .create_announcements_at_date(maturity_time)
            .await
            .expect("Failed to create announcement");

        event_ids.push(event_id);

        // Verify the event ID matches what we expected
        assert_eq!(announcements.pop().unwrap().oracle_event.event_id, event_id);
    }

    event_ids
}

/// Create a test server with populated database and mocked pricefeed
///
/// This creates an Actix test server with announcements in the database
/// and mocked pricefeed data for testing
pub async fn get_populated_test_server(
    announcement_count: usize,
    pool: PgPool,
) -> (actix_test::TestServer, Vec<EventId>) {
    let channel_sender = broadcast::Sender::new(32);
    let (context_handler, mut oracle_context) = MockContext::new(pool).await;

    // Populate the database with announcements (this also sets up mocked pricefeed data)
    let event_ids =
        populate_test_db(&mut oracle_context, announcement_count, &context_handler).await;

    let api_context = ApiContext {
        oracle_context,
        offset_duration: chrono::Duration::minutes(2),
        channel_sender,
    };

    let _ = context_handler.send(event_ids.iter().map(|&e| (e, Some(50000.0))).collect());

    (
        actix_test::start(move || {
            let factory = v1_app_factory::<MockContext>(true);

            App::new().app_data(api_context.clone()).service(factory)
        }),
        event_ids,
    )
}

/// Create a test server with the WebSocket endpoint and mocked pricefeed
///
/// This creates an Actix test server with our API context and WebSocket route
/// and sets up some basic mocked pricefeed data for testing
pub async fn get_test_server(pool: PgPool) -> (HandlerToMock, MockContext, actix_test::TestServer) {
    let channel_sender = broadcast::Sender::new(32);
    let (context_handler, context) = MockContext::new(pool).await;

    // Set up some basic mocked pricefeed data for testing
    let now = Utc::now()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, now);
    let _ = context_handler.send(vec![(event_id, Some(50000.0))]);

    let oracle = context.oracles().get(&AssetPair::BtcUsd).unwrap();
    oracle.create_announcements_at_date(now).await.unwrap();

    let api_context = ApiContext {
        oracle_context: context.clone(),
        offset_duration: chrono::Duration::minutes(2),
        channel_sender,
    };

    (
        context_handler,
        context,
        actix_test::start(move || {
            let factory = v1_app_factory::<MockContext>(true);

            App::new().app_data(api_context.clone()).service(factory)
        }),
    )
}

/// Create a JSON-RPC get request for testing
///
/// Creates a request to get a BTC/USD attestation event using a real event ID
/// from the database if available.
fn create_get_request(event_id: EventId) -> String {
    let request = Request {
        jsonrpc: Version::V2,
        method: "get".to_string(),
        params: Some(RequestContent::Get(GetRequest {
            event_id,
            asset_pair: EventChannel {
                asset_pair: AssetPair::BtcUsd,
                ty: EventType::Announcement,
            },
        })),
        id: None,
    };

    serde_json::to_string(&request).unwrap()
}

/// Helper function to receive the next non-ping WebSocket message
///
/// This function filters out ping frames and returns the first non-ping message,
/// which is usually what we're interested in for testing.
async fn receive_next_non_ping(
    ws: &mut Framed<BoxedSocket, Codec>,
) -> Result<Frame, PythiaApiError> {
    match ws
        .by_ref()
        .filter(|msg| future::ready(!matches!(msg, Ok(Frame::Ping(_)))))
        .next()
        .await
    {
        Some(Ok(frame)) => Ok(frame),
        Some(Err(e)) => Err(PythiaApiError::WebSocketError(e.to_string())),
        None => Err(PythiaApiError::WebSocketError(
            "WebSocket closed".to_string(),
        )),
    }
}

/// Test basic connection to the WebSocket endpoint
///
/// This test verifies that we can connect to the WebSocket endpoint
/// and receive the expected 101 Switching Protocols status code.
#[sqlx::test]
async fn test_ws_connection(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = Client::default();

        // Verify we can connect to the WebSocket endpoint
        let ws = client.ws(srv.url("/v1/ws")).connect().await.unwrap();

        // Status 101 "Switching Protocols" is the correct response for WebSocket upgrade
        assert_eq!(
            ws.0.status().as_u16(),
            101,
            "Expected WebSocket upgrade status 101, got: {}",
            ws.0.status()
        );
    })
    .await
}

/// Test ping-pong exchange with the WebSocket server
///
/// This test verifies that the server responds to ping messages with pong messages
#[sqlx::test]
async fn test_ws_ping_pong(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = Client::default();

        // Connect to the WebSocket
        let (_, mut ws) = client
            .ws(srv.url("/v1/ws"))
            .connect()
            .await
            .expect("Failed to connect to WebSocket");

        // Send a ping message
        ws.send(Message::Ping("ping test".into()))
            .await
            .expect("Failed to send ping");

        // Receive the response and verify it's a pong
        let resp = receive_next_non_ping(&mut ws).await.unwrap();

        match resp {
            Frame::Pong(bytes) => {
                assert_eq!(
                    bytes, "ping test",
                    "Pong message content doesn't match ping"
                );
            }
            _ => panic!("Expected Pong message, got: {resp:?}"),
        }
    })
    .await
}

/// Test subscription to events
///
/// This test verifies that we can subscribe to events and receive a confirmation
#[sqlx::test]
async fn test_ws_subscription(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = Client::default();

        // Connect to the WebSocket
        let (_, mut ws) = client
            .ws(srv.url("/v1/ws"))
            .connect()
            .await
            .expect("Failed to connect to WebSocket");

        // Send a subscription request

        let request_id = 1337;
        let request = Request {
            jsonrpc: Version::V2,
            method: "subscribe".to_string(),
            params: Some(RequestContent::Subscription(EventChannel {
                asset_pair: AssetPair::BtcUsd,
                ty: EventType::Announcement,
            })),
            id: Some(Id::Num(request_id)),
        };

        let subscription_request = serde_json::to_string(&request).unwrap();

        ws.send(Message::Text(subscription_request.into()))
            .await
            .expect("Failed to send subscription");

        // Verify subscription confirmation
        let resp = receive_next_non_ping(&mut ws).await.unwrap();

        match resp {
            Frame::Text(text) => {
                let response_text = String::from_utf8(text.to_vec()).expect("Invalid UTF-8");

                assert!(
                    response_text.contains(&format!("\"id\": {request_id}")),
                    "Cannot find id of the sent request in response"
                );
                assert!(
                    response_text.contains("Successfully subscribe"),
                    "Expected subscription confirmation, got: {response_text}"
                );
            }
            _ => panic!("Expected Text message, got: {resp:?}"),
        }
    })
    .await
}

/// Test get request for event data with real announcements
///
/// This test populates the database with real announcements and then
/// attempts to get one of them through the WebSocket API.
#[sqlx::test]
async fn test_ws_get_request_existed_event_id(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        // Create a server with 3 test announcements
        let (srv, event_ids) = get_populated_test_server(3, pool).await;
        let client = Client::default();

        // Connect to the WebSocket
        let (_, mut ws) = client
            .ws(srv.url("/v1/ws"))
            .connect()
            .await
            .expect("Failed to connect to WebSocket");

        // Send a get request for an existed event
        let get_request = create_get_request(event_ids[0]);
        ws.send(Message::Text(get_request.into()))
            .await
            .expect("Failed to send get request");

        let resp = receive_next_non_ping(&mut ws).await.unwrap();

        match resp {
            Frame::Text(text) => {
                let response_text = String::from_utf8(text.to_vec()).expect("Invalid UTF-8");
                assert!(
                    response_text.contains(event_ids[0].as_ref()),
                    "Expected {} in response, got: {}",
                    event_ids[0],
                    response_text
                );
            }
            _ => panic!("Expected Text message, got: {resp:?}"),
        }
    })
    .await
}

/// Test get request for event data
///
/// This test verifies that we get an appropriate error response when requesting
/// an event that doesn't exist (since we're using a mock Oracle)
#[sqlx::test]
async fn test_ws_get_request_not_existed_event_id(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = Client::default();

        // Connect to the WebSocket
        let (_, mut ws) = client
            .ws(srv.url("/v1/ws"))
            .connect()
            .await
            .expect("Failed to connect to WebSocket");

        // Send a get request
        let get_request = create_get_request("btc_usd1746003000".parse().unwrap());
        ws.send(Message::Text(get_request.into()))
            .await
            .expect("Failed to send get request");

        // Verify the response - we should get a "eventId not found" response for attestation
        // because we only created the announcements, not attestations
        let resp = receive_next_non_ping(&mut ws).await.unwrap();

        match resp {
            Frame::Text(text) => {
                let response_text = String::from_utf8(text.to_vec()).expect("Invalid UTF-8");
                assert!(
                    response_text.contains("eventId not found")
                        || response_text
                            .contains("method unknown or no oracle set for this asset pair"),
                    "Expected error response, got: {response_text}"
                );
            }
            _ => panic!("Expected Text message, got: {resp:?}"),
        }
    })
    .await
}

/// Test clean WebSocket closure
///
/// This test verifies that we can properly close the WebSocket connection
#[sqlx::test]

async fn test_ws_clean_closure(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        let (_, _, srv) = get_test_server(pool).await;
        let client = Client::default();

        // Connect to the WebSocket
        let (_, mut ws) = client
            .ws(srv.url("/v1/ws"))
            .connect()
            .await
            .expect("Failed to connect to WebSocket");

        // Send a close frame
        ws.send(Message::Close(Some(actix_ws::CloseReason {
            code: actix_ws::CloseCode::Normal,
            description: Some("Test complete".into()),
        })))
        .await
        .expect("Failed to send close frame");

        // May or may not get a close frame in response depending on implementation
        // We'll just check if we get anything back without asserting
        let _ = ws.next().await.unwrap();
    })
    .await
}

// ============================================================================
// HTTP API Tests with Mocked Pricefeed
// ============================================================================

use serde_json::json;

/// Test GET /assets endpoint
///
/// This test verifies that the assets endpoint returns the list of supported asset pairs
#[sqlx::test]
async fn test_http_get_assets(pool: PgPool) -> JoinResult {
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
async fn test_http_get_config(pool: PgPool) -> JoinResult {
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
async fn test_http_get_public_key(pool: PgPool) -> JoinResult {
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
async fn test_http_get_announcement(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        // Create a server with pre-populated announcements
        let (srv, event_ids) = get_populated_test_server(1, pool).await;
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

        let announcement: serde_json::Value = resp.json().await.expect("Failed to parse JSON");
        assert!(
            announcement.get("oracleEvent").is_some(),
            "Expected oracle event"
        );
        assert!(
            announcement.get("announcementSignature").is_some(),
            "Expected announcement signature"
        );
    })
    .await
}

/// Test GET /asset/{asset_pair}/{event_type}/{rfc3339_time} endpoint for attestation
///
/// This test verifies that the attestation endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_get_attestation(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        // For attestation testing, we'll use the force endpoint to create an attestation
        // and then test retrieving it
        let (_, _, srv) = get_test_server(pool).await;
        let client = awc::Client::default();

        // First, create an attestation using the force endpoint
        let now = Utc::now();
        let request_body = json!({
            "maturation": now.to_rfc3339(),
            "price": 50000.0
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
        let _event_id = force_response["attestation"]["eventId"]
            .as_str()
            .expect("Expected event ID");

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

        let attestation: serde_json::Value = resp.json().await.expect("Failed to parse JSON");
        assert!(attestation.get("eventId").is_some(), "Expected event ID");
        assert!(
            attestation.get("signatures").is_some(),
            "Expected signatures"
        );
        assert!(attestation.get("values").is_some(), "Expected values");
    })
    .await
}

/// Test POST /asset/{asset_pair}/announcements/batch endpoint
///
/// This test verifies that the batch announcements endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_batch_announcements(pool: PgPool) -> JoinResult {
    run_in_local_set(async move {
        // Create a server with pre-populated announcements
        let (srv, event_ids) = get_populated_test_server(2, pool).await;
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

/// Test POST /force endpoint
///
/// This test verifies that the force endpoint works with mocked pricefeed
#[sqlx::test]
async fn test_http_force_attestation(pool: PgPool) -> JoinResult {
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
async fn test_http_invalid_asset_pair(pool: PgPool) -> JoinResult {
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
async fn test_http_invalid_timestamp(pool: PgPool) -> JoinResult {
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
async fn test_http_future_attestation(pool: PgPool) -> JoinResult {
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
async fn test_http_with_populated_db_and_mocked_pricefeed(pool: PgPool) -> JoinResult {
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

        let announcement: OracleAnnouncement =
            serde_json::from_slice(&body).expect("Failed to parse JSON");

        announcement.validate(&SECP).unwrap();
        assert_eq!(announcement.oracle_event.event_id, event_id.to_string());

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

        let _ = context_handler.send(vec![(event_id, Some(50000.0))]);

        let _ = oracle_context
            .oracles()
            .get(&AssetPair::BtcUsd)
            .unwrap()
            .attest_at_date(now)
            .await
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

        let attestation: AttestationResponse = resp.json().await.expect("Failed to parse JSON");

        let attestation = OracleAttestation {
            event_id: attestation.event_id.to_string(),
            oracle_public_key: oracle_context
                .oracles()
                .get(&AssetPair::BtcUsd)
                .unwrap()
                .get_public_key()
                .into(),
            signatures: attestation.signatures,
            outcomes: attestation.values.iter().map(|o| o.to_string()).collect(),
        };

        attestation.validate(&SECP, &announcement).unwrap();
        assert_eq!(&*attestation.event_id, &event_id.to_string());
    })
    .await
}
