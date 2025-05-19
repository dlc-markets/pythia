use actix_web::{web, HttpRequest, HttpResponse, Result};
use actix_ws::{Message, MessageStream, Session};
use dlc_messages::oracle_msgs::OracleAnnouncement;
use futures::StreamExt;
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string_pretty};
use std::time::{Duration, Instant};
use tokio::{select, time};
use tokio_stream::wrappers::BroadcastStream;

use super::{error::PythiaApiError, EventNotification, EventType};
use crate::{
    api::{AttestationResponse, EventChannel, GetRequest},
    config::AssetPair,
    oracle::{error::OracleError, Oracle},
    schedule_context::{api_context::ApiContext, OracleContext},
};

#[derive(Clone, Serialize, Debug)]
#[serde(untagged)]
pub(crate) enum EventData {
    Announcement(OracleAnnouncement),
    Attestation(Option<AttestationResponse>),
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub(crate) enum RequestContent {
    Get(GetRequest),
    Subscription(EventChannel),
}

#[derive(Serialize)]
pub(crate) struct EventBroadcastContent {
    channel: Box<str>,
    data: EventData,
}

/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Start the websocket connection
/// with request: `GET /ws`
pub async fn websocket<Context>(
    context: ApiContext<Context>,
    stream: web::Payload,
    req: HttpRequest,
) -> super::Result<HttpResponse>
where
    Context: OracleContext + Unpin + 'static,
{
    let (response, session, msg_stream) = actix_ws::handle(&req, stream)?;

    // Instead of spawning a new task, we'll create a future and return the response
    actix::spawn(handle_websocket_session(session, msg_stream, context));

    Ok(response)
}

async fn handle_websocket_session<Context>(
    mut session: Session,
    mut msg_stream: MessageStream,
    context: ApiContext<Context>,
) where
    Context: OracleContext + Unpin + 'static,
{
    // A client is by default subscribing to the channel of btcusd attestation
    // if such oracle is available
    let mut subscribed_to = Vec::with_capacity(2);
    if context.get_oracle(&AssetPair::BtcUsd).is_some() {
        subscribed_to.push(EventChannel {
            asset_pair: AssetPair::BtcUsd,
            ty: EventType::Attestation,
        });
    }

    // Set up heartbeat and timeout tracking
    let mut last_heartbeat = Instant::now();

    // Create a stream from event broadcast
    let mut broadcast_stream = BroadcastStream::from(context.channel_sender.subscribe());

    // Start the heartbeat interval
    let mut heartbeat_interval = time::interval(HEARTBEAT_INTERVAL);
    // Main event loop
    loop {
        // let heartbeat_tick_future = std::pin::pin!(heartbeat_interval.tick());
        // let msg_stream_future = std::pin::pin!(msg_stream.next());
        select! {
            // Handle heartbeat interval
            _ = heartbeat_interval.tick() => {
                // Check client heartbeats
                if Instant::now().duration_since(last_heartbeat) > CLIENT_TIMEOUT {
                    // Heartbeat timed out
                    println!("Websocket Client heartbeat failed, disconnecting!");
                    break;
                }

                // Send ping
                if session.ping(b"").await.is_err() {
                    break;
                }
            }

            // Handle incoming websocket messages
            Some(msg) = msg_stream.next() => {
                match msg {
                    Ok(msg) => {
                        match msg {
                            Message::Ping(msg) => {
                                last_heartbeat = Instant::now();
                                if session.pong(&msg).await.is_err() {
                                    break;
                                }
                            }
                            Message::Pong(_) => {
                                last_heartbeat = Instant::now();
                            }
                            Message::Text(text) => {
                                last_heartbeat = Instant::now();
                                if handle_text_message(&text, &context, &mut session, &mut subscribed_to).await.is_err() {
                                    break;
                                }
                            }
                            Message::Binary(_) => {
                                last_heartbeat = Instant::now();
                            }
                            Message::Close(reason) => {
                                let _ = session.close(reason).await;
                                break;
                            }
                            _ => break,
                        }
                    }
                    Err(_) => break,
                }
            }

            // Handle events from broadcast
            result = broadcast_stream.next() => {
                if let Some(Ok(event)) = result {
                    if handle_event_notification(&event, &mut session, &subscribed_to).await.is_err() {
                        break;
                    }
                }
            }

            // All channels closed
            else => break,
        }
    }
}

// TODO: replace Box dyn
async fn handle_text_message<Context>(
    text: &str,
    context: &ApiContext<Context>,
    session: &mut Session,
    subscribed_to: &mut Vec<EventChannel>,
) -> Result<(), PythiaApiError>
where
    Context: OracleContext + Unpin + 'static,
{
    let request: Result<JRpcRequest, serde_json::Error> = from_str(text);

    let request = match request {
        Ok(serialized_request) => serialized_request,
        Err(e) => {
            info!("WS: received invalid JSONRPC request: {}", &e);
            session
                .text(format!("Received invalid JSON-RPC request: {}", &e))
                .await?;
            return Ok(());
        }
    };

    let Some(params) = request.params.clone() else {
        session
            .text("JSON-RPC request must have parameters")
            .await?;
        return Ok(());
    };

    match params {
        RequestContent::Get(get_request) => {
            let asset_pair = match get_request.asset_pair {
                EventChannel {
                    asset_pair,
                    ty: EventType::Announcement,
                } => asset_pair,
                EventChannel {
                    asset_pair,
                    ty: EventType::Attestation,
                } => asset_pair,
            };

            match context.get_oracle(&asset_pair) {
                Some(oracle) => {
                    let result = future_oracle_state(oracle.clone(), get_request).await;

                    match result {
                        Ok(result) => {
                            session
                                .text(
                                    to_string_pretty(&jsonrpc_event_response(&request, result))
                                        .expect("JSONRPC Response can always be parsed"),
                                )
                                .await?;
                        }
                        Err(e) => {
                            session
                                .text(
                                    to_string_pretty(&jsonrpc_error_response(
                                        &request,
                                        Some(e.to_string()),
                                    ))
                                    .expect("JSONRPC Response can always be parsed"),
                                )
                                .await?;
                        }
                    }
                }
                None => {
                    session
                        .text(
                            to_string_pretty(&jsonrpc_error_response(&request, None))
                                .expect("JSONRPC Response can always be parsed"),
                        )
                        .await?;
                }
            }
        }
        RequestContent::Subscription(channel) => {
            match request.method.as_str() {
                "subscribe" => {
                    if !subscribed_to.contains(&channel) {
                        subscribed_to.push(channel.clone())
                    }
                }
                "unsubscribe" => {
                    subscribed_to.retain(|c| c != &channel);
                }
                _ => {
                    session
                        .text(
                            to_string_pretty(&jsonrpc_error_response(&request, None))
                                .expect("JSONRPC Response can always be parsed"),
                        )
                        .await?;
                    return Ok(());
                }
            }

            session
                .text(
                    to_string_pretty(&jsonrpc_subscription_response(&request, channel))
                        .expect("JSONRPC Response can always be parsed"),
                )
                .await?;
        }
    }

    Ok(())
}

async fn handle_event_notification(
    event: &EventNotification,
    session: &mut Session,
    subscribed_to: &[EventChannel],
) -> Result<(), PythiaApiError> {
    match event {
        EventNotification::Announcement(asset_pair, _) => {
            if subscribed_to.contains(&EventChannel {
                asset_pair: *asset_pair,
                ty: EventType::Announcement,
            }) {
                session
                    .text(
                        to_string_pretty(&EventBroadcast::from(event.clone()))
                            .expect("serializable response"),
                    )
                    .await?;
            }
        }
        EventNotification::Attestation(asset_pair, _) => {
            if subscribed_to.contains(&EventChannel {
                asset_pair: *asset_pair,
                ty: EventType::Attestation,
            }) {
                session
                    .text(
                        to_string_pretty(&EventBroadcast::from(event.clone()))
                            .expect("serializable response"),
                    )
                    .await?;
            }
        }
    }

    Ok(())
}

async fn future_oracle_state(
    oracle: Oracle,
    request: GetRequest,
) -> Result<Option<EventData>, OracleError> {
    oracle
        .oracle_state(&request.event_id)
        .await
        .map(|state| match (request.asset_pair.ty, state) {
            (EventType::Announcement, Some((announcement, _))) => {
                Some(EventData::Announcement(announcement))
            }
            (EventType::Attestation, Some((_, Some(attestation)))) => Some(EventData::Attestation(
                Some((request.event_id, attestation).into()),
            )),
            (EventType::Attestation, Some((_, None))) => Some(EventData::Attestation(None)),
            (_, None) => None,
        })
}

type EventBroadcast = json_rpc_types::Request<EventBroadcastContent, &'static str>;
type JRpcRequest = json_rpc_types::Request<RequestContent>;
type JRpcResponse = json_rpc_types::Response<EventData, Box<str>, &'static str>;

fn jsonrpc_event_response(
    request: &JRpcRequest,
    event_response: Option<EventData>,
) -> JRpcResponse {
    match (&request.params, event_response) {
        (Some(_), Some(event_response)) => JRpcResponse {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Ok(event_response),
            id: request.id.clone(),
        },
        (_, None) => JRpcResponse {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Err(json_rpc_types::Error {
                code: json_rpc_types::ErrorCode::InvalidParams,
                message: "eventId not found",
                data: None,
            }),
            id: request.id.clone(),
        },
        (None, Some(_)) => unreachable!(),
    }
}

fn jsonrpc_error_response(
    request: &JRpcRequest,
    error: Option<String>,
) -> json_rpc_types::Response<Box<str>, Box<str>, Box<str>> {
    let error = error.unwrap_or("method unknown or no oracle set for this asset pair".to_owned());
    json_rpc_types::Response {
        jsonrpc: json_rpc_types::Version::V2,
        payload: Err(json_rpc_types::Error {
            code: json_rpc_types::ErrorCode::InvalidParams,
            message: error.into_boxed_str(),
            data: None,
        }),
        id: request.id.clone(),
    }
}

fn jsonrpc_subscription_response(
    request: &JRpcRequest,
    channel: EventChannel,
) -> json_rpc_types::Response<String, &str> {
    match channel {
        EventChannel {
            asset_pair,
            ty: EventType::Announcement,
        } => json_rpc_types::Response {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Ok(format!(
                "Successfully {} for announcement of the {} pair",
                request.method, asset_pair
            )),
            id: request.id.clone(),
        },
        EventChannel {
            asset_pair,
            ty: EventType::Attestation,
        } => json_rpc_types::Response {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Ok(format!(
                "Successfully {} for attestation of the {} pair",
                request.method, asset_pair
            )),
            id: request.id.clone(),
        },
    }
}

impl From<EventNotification> for EventBroadcast {
    fn from(value: EventNotification) -> Self {
        json_rpc_types::Request {
            jsonrpc: json_rpc_types::Version::V2,
            method: "subscriptions",
            params: Some(value.into()),
            id: None,
        }
    }
}

impl From<EventNotification> for EventBroadcastContent {
    fn from(value: EventNotification) -> Self {
        let (channel_string, event_data) = match value {
            EventNotification::Announcement(asset_pair, event_data) => (
                asset_pair.to_string().to_lowercase() + "/announcement",
                EventData::Announcement(event_data),
            ),
            EventNotification::Attestation(asset_pair, event_data) => (
                asset_pair.to_string().to_lowercase() + "/attestation",
                EventData::Attestation(Some(event_data)),
            ),
        };

        Self {
            channel: channel_string.into_boxed_str(),
            data: event_data,
        }
    }
}
