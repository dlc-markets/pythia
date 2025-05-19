use crate::config::AssetPair;
use crate::config::AssetPairInfo;
use crate::oracle::postgres::DBconnection;
use crate::oracle::Oracle;
use crate::pricefeeds::ImplementedPriceFeed;
use crate::schedule_context::api_context::ApiContext;
use crate::schedule_context::OracleContextInner;
use actix_codec::Framed;
use actix_web::{web, App};
use actix_ws::Message;
use awc::ws::Codec;
use awc::ws::Frame;
use awc::BoxedSocket;
use awc::Client;
use chrono::{Duration, Utc};
use cron::Schedule;
use dlc_messages::oracle_msgs::DigitDecompositionEventDescriptor;
use futures_util::{SinkExt, StreamExt};
use json_rpc_types::{Id, Request, Version};
use secp256k1_zkp::rand;
use secp256k1_zkp::{Keypair, Secp256k1};
use sqlx::postgres::PgPoolOptions;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::future;
use std::str::FromStr;
use tokio::sync::broadcast;

use super::error::PythiaApiError;
use super::ws::RequestContent;
use super::EventChannel;
use super::EventType;
use super::GetRequest;

/// A mock implementation of OracleContext for testing
#[derive(Clone)]
pub struct MockContext {
    inner: OracleContextInner,
}

impl MockContext {
    async fn new() -> Self {
        // Create a default hourly schedule for tests
        let schedule = Schedule::from_str("0 */1 * * * * *").expect("Valid cron schedule");

        // Create mock asset pair infos for testing
        let mut oracles = HashMap::new();

        // We'll simulate having a BTC/USD oracle similar to how it's done in main.rs
        let btc_usd_asset_pair_info = AssetPairInfo {
            pricefeed: ImplementedPriceFeed::Lnmarkets,
            asset_pair: AssetPair::BtcUsd,
            event_descriptor: DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: false,
                unit: "usd/btc".to_string(),
                precision: 12,
                nb_digits: 20,
            },
        };

        let secp = Secp256k1::new();
        let (secret_key, _) = secp.generate_keypair(&mut rand::thread_rng());
        let keypair = Keypair::from_secret_key(&secp, &secret_key);

        // Create a mock DB connection for testing
        let db_connection = DBconnection::get_test_db_connection().await;

        // Create an Oracle with our mock DB connection
        let btc_usd_oracle = Oracle::new(btc_usd_asset_pair_info, db_connection, keypair);

        // Add the oracle to our map (same as in main.rs)
        oracles.insert(AssetPair::BtcUsd, btc_usd_oracle);

        // Create an inner context
        let inner = OracleContextInner { oracles, schedule };

        Self { inner }
    }
}

impl Borrow<OracleContextInner> for MockContext {
    fn borrow(&self) -> &OracleContextInner {
        &self.inner
    }
}

impl DBconnection {
    /// Create a test database connection for tests
    async fn get_test_db_connection() -> Self {
        // Get the test database URL from environment or use default
        let test_db_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:postgres@127.0.0.1:5433/pythia_test".to_string()
        });

        // Create connection pool
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&test_db_url)
            .await
            .expect("Failed to connect to test database");

        // Clear any data from previous tests
        sqlx::query("TRUNCATE TABLE oracle.events CASCADE")
            .execute(&pool)
            .await
            .expect("Failed to clear events table");

        sqlx::query("TRUNCATE TABLE oracle.digits CASCADE")
            .execute(&pool)
            .await
            .expect("Failed to clear digits table");

        Self(pool)
    }
}

/// Create a MockContext with a test database connection
pub async fn create_test_context() -> MockContext {
    // Create a schedule for tests
    let schedule = Schedule::from_str("0 */1 * * * * *").expect("Valid cron schedule");

    // Create mock asset pair infos for testing
    let mut oracles = HashMap::new();

    // We'll simulate having a BTC/USD oracle similar to how it's done in main.rs
    let btc_usd_asset_pair_info = AssetPairInfo {
        pricefeed: ImplementedPriceFeed::Lnmarkets,
        asset_pair: AssetPair::BtcUsd,
        event_descriptor: DigitDecompositionEventDescriptor {
            base: 2,
            is_signed: false,
            unit: "usd/btc".to_string(),
            precision: 12,
            nb_digits: 20,
        },
    };

    let secp = Secp256k1::new();
    let (secret_key, _) = secp.generate_keypair(&mut rand::thread_rng());
    let keypair = Keypair::from_secret_key(&secp, &secret_key);

    // Create a test DB connection
    let db_connection = DBconnection::get_test_db_connection().await;

    // Create an Oracle with our test DB connection
    let btc_usd_oracle = Oracle::new(btc_usd_asset_pair_info, db_connection, keypair);

    oracles.insert(AssetPair::BtcUsd, btc_usd_oracle);

    // Create an inner context
    let inner = OracleContextInner { oracles, schedule };

    MockContext { inner }
}

/// Populates the database with announcement events for testing
///
/// Creates a specified number of announcements with different maturity times
/// and stores their event IDs for later use in tests.
pub async fn populate_test_db(mock_context: &mut MockContext, count: usize) -> Vec<String> {
    let now = Utc::now();

    // Create a vector to store event IDs
    let mut event_ids = Vec::with_capacity(count);

    // Find the BTC/USD oracle
    let oracle = mock_context
        .inner
        .oracles
        .get(&AssetPair::BtcUsd)
        .expect("BTC/USD oracle must exist");

    // Create announcements at different times
    for i in 0..count {
        let maturity_time = now + Duration::hours(i as i64 + 1);

        // Create announcement
        let announcement = oracle
            .create_announcement(maturity_time)
            .await
            .expect("Failed to create announcement");

        // Store the event ID
        event_ids.push(announcement.oracle_event.event_id.to_string());

        println!(
            "Created test announcement: {}",
            announcement.oracle_event.event_id
        );
    }

    event_ids
}

/// Create a mock API context with populated test database
///
/// This creates a context and populates the test database with announcements
async fn create_populated_test_api_context(
    announcement_count: usize,
) -> (ApiContext<MockContext>, Vec<String>) {
    let (sender, _) = broadcast::channel(32);
    let mut context = create_test_context().await;

    // Populate the database with announcements
    let event_ids = populate_test_db(&mut context, announcement_count).await;

    (
        ApiContext {
            oracle_context: context,
            offset_duration: chrono::Duration::minutes(2),
            channel_sender: sender,
        },
        event_ids,
    )
}

/// Create a test server with populated database
///
/// This creates an Actix test server with announcements in the database
pub async fn get_populated_test_server(
    announcement_count: usize,
) -> (actix_test::TestServer, Vec<String>) {
    let (api_context, event_ids) = create_populated_test_api_context(announcement_count).await;

    (
        actix_test::start(move || {
            let factory = web::scope("/v1").route(
                "/ws",
                web::get().to(crate::api::ws::websocket::<MockContext>),
            );

            App::new().app_data(api_context.clone()).service(factory)
        }),
        event_ids,
    )
}

/// Create a mock API context for testing
///
/// This sets up a broadcast channel and creates a default MockContext
async fn create_test_api_context() -> ApiContext<MockContext> {
    let (sender, _) = broadcast::channel(32);
    let context = MockContext::new().await;

    ApiContext {
        oracle_context: context,
        offset_duration: chrono::Duration::minutes(2),
        channel_sender: sender,
    }
}

/// Create a test server with the WebSocket endpoint
///
/// This creates an Actix test server with our API context and WebSocket route
pub async fn get_test_server() -> actix_test::TestServer {
    let api_context = create_test_api_context().await;

    actix_test::start(move || {
        let factory = web::scope("/v1").route(
            "/ws",
            web::get().to(crate::api::ws::websocket::<MockContext>),
        );

        App::new().app_data(api_context.clone()).service(factory)
    })
}

/// Create a JSON-RPC subscription request for testing
///
/// Creates a request to subscribe to BTC/USD announcements
fn create_subscription_request() -> String {
    let request = Request {
        jsonrpc: Version::V2,
        method: "subscribe".to_string(),
        params: Some(RequestContent::Subscription(EventChannel {
            asset_pair: AssetPair::BtcUsd,
            ty: EventType::Announcement,
        })),
        id: Some(Id::Num(1)),
    };

    serde_json::to_string(&request).unwrap()
}

/// Create a JSON-RPC get request for testing
///
/// Creates a request to get a BTC/USD attestation event using a real event ID
/// from the database if available.
fn create_get_request(event_id: String) -> String {
    let request = Request {
        jsonrpc: Version::V2,
        method: "get".to_string(),
        params: Some(RequestContent::Get(GetRequest {
            event_id: event_id.into_boxed_str(),
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
            "WebSocket returns None".to_string(),
        )),
    }
}

/// Test basic connection to the WebSocket endpoint
///
/// This test verifies that we can connect to the WebSocket endpoint
/// and receive the expected 101 Switching Protocols status code.
#[actix_web::test]
#[serial_test::serial]
async fn test_ws_connection() {
    let srv = get_test_server().await;
    let client = Client::default();

    // Verify we can connect to the WebSocket endpoint
    let ws = client.ws(srv.url("/v1/ws")).connect().await.unwrap();
    println!("WebSocket connection status: {}", ws.0.status());

    // Status 101 "Switching Protocols" is the correct response for WebSocket upgrade
    assert_eq!(
        ws.0.status().as_u16(),
        101,
        "Expected WebSocket upgrade status 101, got: {}",
        ws.0.status()
    );
}

/// Test ping-pong exchange with the WebSocket server
///
/// This test verifies that the server responds to ping messages with pong messages
#[actix_web::test]
#[serial_test::serial]
async fn test_ws_ping_pong() -> Result<(), PythiaApiError> {
    let srv = get_test_server().await;
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
    let resp = receive_next_non_ping(&mut ws).await?;

    match resp {
        Frame::Pong(bytes) => {
            assert_eq!(
                bytes, "ping test",
                "Pong message content doesn't match ping"
            );
        }
        _ => panic!("Expected Pong message, got: {:?}", resp),
    }

    Ok(())
}

/// Test subscription to events
///
/// This test verifies that we can subscribe to events and receive a confirmation
#[actix_web::test]
#[serial_test::serial]
async fn test_ws_subscription() -> Result<(), PythiaApiError> {
    let srv = get_test_server().await;
    let client = Client::default();

    // Connect to the WebSocket
    let (_, mut ws) = client
        .ws(srv.url("/v1/ws"))
        .connect()
        .await
        .expect("Failed to connect to WebSocket");

    // Send a subscription request
    let subscription_request = create_subscription_request();
    ws.send(Message::Text(subscription_request.into()))
        .await
        .expect("Failed to send subscription");

    // Verify subscription confirmation
    let resp = receive_next_non_ping(&mut ws).await?;

    match resp {
        Frame::Text(text) => {
            let response_text = String::from_utf8(text.to_vec()).expect("Invalid UTF-8");
            assert!(
                response_text.contains("Successfully subscribe"),
                "Expected subscription confirmation, got: {}",
                response_text
            );
        }
        _ => panic!("Expected Text message, got: {:?}", resp),
    }
    Ok(())
}

/// Test get request for event data with real announcements
///
/// This test populates the database with real announcements and then
/// attempts to get one of them through the WebSocket API.
#[actix_web::test]
#[serial_test::serial]
async fn test_ws_get_request_existed_event_id() -> Result<(), PythiaApiError> {
    // Create a server with 3 test announcements
    let (srv, event_ids) = get_populated_test_server(3).await;
    let client = Client::default();

    // Connect to the WebSocket
    let (_, mut ws) = client
        .ws(srv.url("/v1/ws"))
        .connect()
        .await
        .expect("Failed to connect to WebSocket");

    // Send a get request for an existed event
    let get_request = create_get_request(event_ids[0].clone());
    println!("Sending get request: {}", get_request);
    ws.send(Message::Text(get_request.into()))
        .await
        .expect("Failed to send get request");

    let resp = receive_next_non_ping(&mut ws).await?;

    match resp {
        Frame::Text(text) => {
            let response_text = String::from_utf8(text.to_vec()).expect("Invalid UTF-8");
            println!("Get response: {}", response_text);
            assert!(
                response_text.contains(&event_ids[0]),
                "Expected error response, got: {}",
                response_text
            );
        }
        _ => panic!("Expected Text message, got: {:?}", resp),
    }
    Ok(())
}

/// Test get request for event data
///
/// This test verifies that we get an appropriate error response when requesting
/// an event that doesn't exist (since we're using a mock Oracle)
#[actix_web::test]
#[serial_test::serial]
async fn test_ws_get_request_not_existed_event_id() -> Result<(), PythiaApiError> {
    let srv = get_test_server().await;
    let client = Client::default();

    // Connect to the WebSocket
    let (_, mut ws) = client
        .ws(srv.url("/v1/ws"))
        .connect()
        .await
        .expect("Failed to connect to WebSocket");

    // Send a get request
    let get_request = create_get_request("btc_usd1746003000".to_string());
    ws.send(Message::Text(get_request.into()))
        .await
        .expect("Failed to send get request");

    // Verify the response - we should get a "eventId not found" response for attestation
    // because we only created the announcements, not attestations
    let resp = receive_next_non_ping(&mut ws).await?;

    match resp {
        Frame::Text(text) => {
            let response_text = String::from_utf8(text.to_vec()).expect("Invalid UTF-8");
            assert!(
                response_text.contains("eventId not found")
                    || response_text
                        .contains("method unknown or no oracle set for this asset pair"),
                "Expected error response, got: {}",
                response_text
            );
        }
        _ => panic!("Expected Text message, got: {:?}", resp),
    }
    Ok(())
}

/// Test clean WebSocket closure
///
/// This test verifies that we can properly close the WebSocket connection
#[actix_web::test]
#[serial_test::serial]
async fn test_ws_clean_closure() {
    let srv = get_test_server().await;
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
    let _ = ws.next().await;
}
