use std::future;

use awc::{
    BoxedSocket, Client,
    ws::{Codec, Frame},
};

use actix_codec::Framed;
use actix_ws::Message;
use sqlx::PgPool;

use futures_util::{SinkExt, StreamExt};
use json_rpc_types::{Id, Request, Version};

use crate::{
    api::error::PythiaApiError,
    data_models::{asset_pair::AssetPair, event_ids::EventId},
    run_in_local_set,
};

use super::{EventChannel, EventType, GetRequest, RequestContent};

use crate::test::api::{get_test_server, populate_test_db};

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
                expiry: None,
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
async fn test_ws_connection(pool: PgPool) {
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
async fn test_ws_ping_pong(pool: PgPool) {
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
async fn test_ws_subscription(pool: PgPool) {
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
                expiry: None,
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
async fn test_ws_get_request_existed_event_id(pool: PgPool) {
    run_in_local_set(async move {
        // Create a server with 3 test announcements
        let (context_handler, mut oracle_context, srv) = get_test_server(pool).await;

        let event_ids = populate_test_db(&mut oracle_context, 3, &context_handler).await;

        let client = Client::default();

        // Connect to the WebSocket
        let (_, mut ws) = client
            .ws(srv.url("/v1/ws"))
            .connect()
            .await
            .expect("Failed to connect to WebSocket");

        // Send a get request for an existed event
        let get_request = create_get_request(event_ids[0].as_event_id());
        ws.send(Message::Text(get_request.into()))
            .await
            .expect("Failed to send get request");

        let resp = receive_next_non_ping(&mut ws).await.unwrap();

        match resp {
            Frame::Text(text) => {
                let response_text = String::from_utf8(text.to_vec()).expect("Invalid UTF-8");
                assert!(
                    response_text.contains(event_ids[0].as_event_id().as_ref()),
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
async fn test_ws_get_request_not_existed_event_id(pool: PgPool) {
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

async fn test_ws_clean_closure(pool: PgPool) {
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
