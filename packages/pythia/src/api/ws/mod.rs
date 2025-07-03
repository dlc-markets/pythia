use actix_web::{web, HttpRequest, HttpResponse, Result};
use actix_ws::{CloseCode, CloseReason, Message, MessageStream, Session};
use chrono::{DateTime, TimeDelta, Utc};
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string_pretty};
use std::time::{Duration, Instant};
use tokio::{select, time};

use crate::{
    data_models::{
        asset_pair::AssetPair, event_ids::EventId, expiries::Expiry, oracle_msgs::Announcement,
    },
    oracle::{error::OracleError, Oracle},
    schedule_context::{api_context::ApiContext, OracleContext},
};

use super::{error::PythiaApiError, AttestationResponse, EventNotification, EventType};

#[cfg(test)]
mod test;

#[derive(Clone, Serialize, Debug)]
#[serde(untagged)]
enum EventData {
    Announcement(Announcement),
    Attestation(Option<AttestationResponse>),
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(untagged)]
enum RequestContent {
    Get(GetRequest),
    Subscription(EventChannel),
}

#[derive(Serialize)]
struct EventBroadcastContent {
    channel: Box<str>,
    data: EventData,
}

/// Channel for expiry, the All channel matches any expiry
#[derive(PartialEq, Deserialize, Serialize, Clone, Copy)]
#[serde(untagged)]
enum ExpiryChannel {
    #[serde(with = "all_variant")]
    All,
    Expiry(Expiry),
}

mod all_variant {
    use serde::de::Visitor;

    pub fn serialize<S>(serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("ALL")
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<(), D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct AllVariantVisitor;

        impl<'de> Visitor<'de> for AllVariantVisitor {
            type Value = ();

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("ALL")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    "ALL" | "all" | "All" | "ANY" | "any" | "Any" => Ok(()),
                    _ => Err(E::custom("Invalid expiry")),
                }
            }
        }
        deserializer.deserialize_str(AllVariantVisitor)
    }
}

#[derive(PartialEq, Deserialize, Serialize, Clone, Copy)]
struct EventChannel {
    #[serde(rename = "assetPair")]
    asset_pair: AssetPair,
    #[serde(rename = "type")]
    ty: EventType,
    expiry: Option<ExpiryChannel>,
}

impl EventChannel {
    fn matches_with(&self, other: &Self) -> bool {
        self.asset_pair == other.asset_pair
            && self.ty == other.ty
            && ((self.expiry == other.expiry)
                || (self.expiry == Some(ExpiryChannel::All) && other.expiry.is_some()))
    }
}

#[derive(Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct GetRequest {
    #[serde(flatten)]
    asset_pair: EventChannel,
    event_id: EventId,
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
    subscribed_to.push(EventChannel {
        asset_pair: AssetPair::BtcUsd,
        ty: EventType::Attestation,
        expiry: None,
    });

    // Set up heartbeat and timeout tracking
    let mut last_heartbeat = Instant::now();

    // Create a stream from event broadcast
    let mut channel_receiver = context.channel_sender.subscribe();

    // Start the heartbeat interval
    let mut heartbeat_interval = time::interval(HEARTBEAT_INTERVAL);
    // Main event loop
    loop {
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
            Some(payload) = msg_stream.recv() => {
                let msg = match payload {
                    Ok(msg) => msg,
                    Err(e) => {
                        error!("WS: closing connection on protocol error: {}", &e);
                        let _ = session.close(Some(CloseReason {
                            code: CloseCode::Protocol,
                            description: Some(format!("Protocol error: {}", &e)),
                        })).await;
                        break;
                    }
                };

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

            // Handle events from broadcast
            Ok(event) = channel_receiver.recv() => {
                if handle_event_notification(event, &mut session, &mut subscribed_to).await.is_err() {
                    break;
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
    let request: JRpcRequest = match from_str(text) {
        Ok(request) => request,
        Err(e) => {
            info!("WS: received invalid JSONRPC request: {}", &e);
            return Ok(session
                .text(format!("Received invalid JSON-RPC request: {}", &e))
                .await?);
        }
    };

    let Some(params) = request.params.as_ref() else {
        session
            .text("JSON-RPC request must have parameters")
            .await?;
        return Ok(());
    };

    match params {
        RequestContent::Get(get_request) => {
            let response = match context.get_oracle(&get_request.asset_pair.asset_pair) {
                Some(oracle) => match future_oracle_state(oracle, get_request).await {
                    Ok(result) => to_string_pretty(&jsonrpc_event_response(&request, result)),
                    Err(e) => {
                        to_string_pretty(&jsonrpc_error_response(&request, Some(e.to_string())))
                    }
                },
                None => to_string_pretty(&jsonrpc_error_response(&request, None)),
            };

            session
                .text(response.expect("JSONRPC Response can always be parsed"))
                .await?;
        }
        RequestContent::Subscription(channel) => {
            if let Some(ExpiryChannel::Expiry(expiry)) = channel.expiry {
                let end_of_channel_date = DateTime::<Utc>::from(expiry)
                    - if channel.ty == EventType::Announcement {
                        context.offset_duration
                    } else {
                        TimeDelta::zero()
                    };
                if end_of_channel_date < Utc::now() {
                    session
                        .text(
                            to_string_pretty(&jsonrpc_error_response(
                                &request,
                                Some(
                                    "Subscription to this channel is not allowed as it will not emit any events"
                                        .to_string(),
                                ),
                            ))
                            .expect("JSONRPC Response can always be parsed"),
                        )
                        .await?;
                    return Ok(());
                }
            }

            match request.method.as_str() {
                "subscribe" => {
                    if !subscribed_to.contains(channel) {
                        subscribed_to.push(*channel)
                    }
                }
                "unsubscribe" => {
                    subscribed_to.retain(|c| c != channel);
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

            let channel = *channel;

            let response = to_string_pretty(&jsonrpc_subscription_response(request, channel));

            session
                .text(response.expect("JSONRPC Response can always be parsed"))
                .await?;
        }
    }

    Ok(())
}

async fn handle_event_notification(
    event: EventNotification,
    session: &mut Session,
    subscribed_to: &mut Vec<EventChannel>,
) -> Result<(), PythiaApiError> {
    let (asset_pair, ty, expiry, event_id) = match event {
        EventNotification::Announcement(asset_pair, expiry, ref announcement) => (
            asset_pair,
            EventType::Announcement,
            expiry,
            announcement.oracle_event.event_id,
        ),
        EventNotification::Attestation(asset_pair, expiry, ref attestation) => (
            asset_pair,
            EventType::Attestation,
            expiry,
            attestation.event_id,
        ),
    };

    let channel = EventChannel {
        asset_pair,
        ty,
        expiry: expiry.map(ExpiryChannel::Expiry),
    };

    if subscribed_to.iter().any(|c| c.matches_with(&channel)) {
        session
            .text(to_string_pretty(&EventBroadcast::from(event)).expect("serializable response"))
            .await?;

        if let EventId::Delivery(_) = event_id {
            subscribed_to.retain(|c| c != &channel);

            let type_str = match channel.ty {
                EventType::Announcement => "announcements",
                EventType::Attestation => "attestations",
            };

            session.text(to_string_pretty(&json_rpc_types::Request::<_, &'static str> {
                jsonrpc: json_rpc_types::Version::V2,
                method: "unsubscribed",
                params: Some(format!(
                    "Unsubscribed from {} for the forward with expiry {} of the {} pair as delivery {} has just been notified",
                    type_str,
                    expiry.expect("eventId is delivery so it has an expiry"),
                    asset_pair,
                    &type_str[..type_str.len() - 1],
                )),
                id: None,
            }).expect("JSONRPC Request can always be parsed"),
            ).await?;
        }
    }

    Ok(())
}

async fn future_oracle_state(
    oracle: &Oracle,
    request: &GetRequest,
) -> Result<Option<EventData>, OracleError> {
    oracle
        .oracle_state(request.event_id)
        .await
        .map(|state| match (&request.asset_pair.ty, state) {
            (EventType::Announcement, Some((announcement, _))) => {
                Some(EventData::Announcement(announcement))
            }
            (EventType::Attestation, Some((_, Some(attestation)))) => {
                Some(EventData::Attestation(Some(attestation.into())))
            }
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
    request: JRpcRequest,
    channel: EventChannel,
) -> json_rpc_types::Response<String, ()> {
    let EventChannel {
        asset_pair,
        ty,
        expiry,
    } = channel;
    json_rpc_types::Response {
        jsonrpc: json_rpc_types::Version::V2,
        payload: Ok(format!(
            "Successfully {} for {ty} of the {asset_pair} pair",
            request.method,
        ) + &expiry
            .map(|e| {
                if let ExpiryChannel::Expiry(expiry) = e {
                    format!(" for the forward with expiry {expiry}")
                } else {
                    " for all forwards".to_string()
                }
            })
            .unwrap_or("".to_string())),
        id: request.id,
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
            EventNotification::Announcement(asset_pair, expiry, event_data) => (
                asset_pair.to_string().to_lowercase()
                    + &expiry
                        .map(|s| "/".to_string() + s.as_ref())
                        .unwrap_or("".to_string())
                    + "/announcement",
                EventData::Announcement(event_data),
            ),
            EventNotification::Attestation(asset_pair, expiry, event_data) => (
                asset_pair.to_string().to_lowercase()
                    + &expiry
                        .map(|s| "/".to_string() + s.as_ref())
                        .unwrap_or("".to_string())
                    + "/attestation",
                EventData::Attestation(Some(event_data)),
            ),
        };

        Self {
            channel: channel_string.into_boxed_str(),
            data: event_data,
        }
    }
}
