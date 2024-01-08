use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use actix::{fut::wrap_future, prelude::*};
use actix_web_actors::ws;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string_pretty};
use tokio::sync::broadcast::Receiver;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};

use crate::{api::AttestationResponse, common::AssetPair, oracle::Oracle};

/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// websocket connection is long running connection, it easier
/// to handle with an actor
pub struct PythiaWebSocket {
    /// Client must send ping at least once per 10 seconds (CLIENT_TIMEOUT),
    /// otherwise we drop connection.
    hb: Instant,
    /// The Oracles instances the websocket allow to interact with
    oracles: Arc<HashMap<AssetPair, Oracle>>,
    /// The stream of broadcasted event for the client
    event_rx: ReceiverHandle,
    /// Subscription option to channels
    subscribed_to: Vec<EventChannel>,
}

impl PythiaWebSocket {
    pub fn new(oracles: Arc<HashMap<AssetPair, Oracle>>, event_rx: ReceiverHandle) -> Self {
        let mut subscription_vec = Vec::with_capacity(2);
        // A client is by default subscribing to the channel of btcusd attestation
        subscription_vec.push(EventChannel::Attestation {
            asset_pair: AssetPair::Btcusd,
        });
        Self {
            hb: Instant::now(),
            oracles,
            event_rx,
            subscribed_to: subscription_vec,
        }
    }

    /// helper method that sends ping to client every 5 seconds (HEARTBEAT_INTERVAL).
    ///
    /// also this method checks heartbeats from client
    fn hb(&self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            // check client heartbeats
            if Instant::now().duration_since(act.hb) > CLIENT_TIMEOUT {
                // heartbeat timed out
                println!("Websocket Client heartbeat failed, disconnecting!");

                // stop actor
                ctx.stop();

                // don't try to send a ping
                return;
            }

            ctx.ping(b"");
        });
    }
}

impl Actor for PythiaWebSocket {
    type Context = ws::WebsocketContext<Self>;

    /// Method is called on actor start. We start the heartbeat process and websocket here.
    fn started(&mut self, ctx: &mut Self::Context) {
        self.hb(ctx);
        // Attach the stream of attestations to the websocket
        ctx.add_stream(BroadcastStream::from(self.event_rx.clone().0));
    }
}

async fn future_oracle_state(oracle: Oracle, request: GetRequest) -> Option<EventData> {
    let state = oracle.oracle_state(&request.event_id).await.unwrap();
    match (request.asset_pair, state) {
        (EventChannel::Announcement { asset_pair: _ }, Some((announcement, _))) => {
            Some(EventData::Announcement(announcement))
        }
        (EventChannel::Attestation { asset_pair: _ }, Some((_, Some(attestation)))) => Some(
            EventData::Attestation(Some((request.event_id, attestation).into())),
        ),
        (EventChannel::Attestation { asset_pair: _ }, Some((_, None))) => {
            Some(EventData::Attestation(None))
        }
        (_, None) => None,
    }
}

/// Handler for `ws::Message`
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for PythiaWebSocket {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => {
                self.hb = Instant::now();
                ctx.pong(&msg);
            }
            Ok(ws::Message::Pong(_)) => {
                self.hb = Instant::now();
            }
            Ok(ws::Message::Text(request)) => {
                let request: Result<JRpcRequest, serde_json::Error> = from_str(request.as_ref());
                let request = match request {
                    Ok(serialized_request) => serialized_request,
                    Err(e) => {
                        info!("WS: received invalid JSONRPC request: {}", &e);
                        ctx.text(format!("Received invalid JSON-RPC request: {}", &e));
                        return;
                    }
                };

                let Some(params) = request.params.clone() else {
                    ctx.text("JSON-RPC request must have parameters");
                    return;
                };

                let future_state = match params {
                    RequestContent::Get(get_request) => {
                        let asset_pair = match get_request.asset_pair {
                            EventChannel::Announcement { asset_pair } => asset_pair,
                            EventChannel::Attestation { asset_pair } => asset_pair,
                        };
                        match self.oracles.get(&asset_pair) {
                            Some(oracle) => future_oracle_state(oracle.clone(), get_request),
                            None => {
                                ctx.text(
                                    to_string_pretty(&jsonrpc_error_response(&request, None))
                                        .expect("JSONRPC Response can always be parsed"),
                                );
                                return;
                            }
                        }
                    }
                    RequestContent::Subscription(channel) => {
                        match request.method.as_str() {
                            "subscribe" => {
                                if !self.subscribed_to.contains(&channel) {
                                    self.subscribed_to.push(channel.clone())
                                }
                            }
                            "unsubscribe" => {
                                self.subscribed_to.retain(|c| c != &channel);
                            }
                            _ => {
                                ctx.text(
                                    to_string_pretty(&jsonrpc_error_response(&request, None))
                                        .expect("JSONRPC Response can always be parsed"),
                                );
                                return;
                            }
                        }
                        ctx.text(
                            to_string_pretty(&jsonrpc_subscription_response(&request, channel))
                                .expect("JSONRPC Response can always be parsed"),
                        );
                        return;
                    }
                };

                // https://stackoverflow.com/questions/72068485/how-use-postgres-deadpool-postgres-with-websocket-actix-actix-web-actors
                let fut = wrap_future::<_, Self>(future_state);
                let fut = fut.map(move |result, _actor, ctx| {
                    ctx.text(
                        to_string_pretty(&jsonrpc_event_response(&request, result))
                            .expect("JSONRPC Response can always be parsed"),
                    );
                });
                ctx.spawn(fut);
                // future_state.into_actor(self).spawn(ctx);
            }
            Ok(ws::Message::Binary(_bin)) => (),
            Ok(ws::Message::Close(reason)) => {
                ctx.close(reason);
                ctx.stop();
            }
            _ => ctx.stop(),
        }
    }
}

/// Handle produced attestation and send them to clients
impl StreamHandler<Result<EventNotification, BroadcastStreamRecvError>> for PythiaWebSocket {
    fn handle(
        &mut self,
        item: Result<EventNotification, BroadcastStreamRecvError>,
        ctx: &mut Self::Context,
    ) {
        let event = item.expect("attestation are rare enough it should not lag behind");

        match &event {
            EventNotification::Announcement(asset_pair, _) => {
                if self.subscribed_to.contains(&EventChannel::Announcement {
                    asset_pair: *asset_pair,
                }) {
                    ctx.text(
                        to_string_pretty(&EventBroadcast::from(event))
                            .expect("serializable response"),
                    );
                }
            }
            EventNotification::Attestation(asset_pair, _) => {
                if self.subscribed_to.contains(&EventChannel::Attestation {
                    asset_pair: *asset_pair,
                }) {
                    ctx.text(
                        to_string_pretty(&EventBroadcast::from(event))
                            .expect("serializable response"),
                    );
                }
            }
        }
    }
}

type EventBroadcast = json_rpc_types::Request<EventBroadcastContent, &'static str>;

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

#[derive(PartialEq, Deserialize, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
enum EventChannel {
    Announcement { asset_pair: AssetPair },
    Attestation { asset_pair: AssetPair },
}
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GetRequest {
    #[serde(flatten)]
    asset_pair: EventChannel,
    event_id: Box<str>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged, rename_all = "camelCase")]
enum RequestContent {
    Get(GetRequest),
    Subscription(EventChannel),
}

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
    error: Option<serde_json::error::Error>,
) -> json_rpc_types::Response<Box<str>, Box<str>, Box<str>> {
    let error = error
        .map(|e| e.to_string())
        .unwrap_or("method unknown or no oracle set for this asset pair".to_owned());
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
        EventChannel::Announcement { asset_pair } => json_rpc_types::Response {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Ok(format!(
                "Successfully {} for announcement of the {} pair",
                request.method, asset_pair
            )),
            id: request.id.clone(),
        },
        EventChannel::Attestation { asset_pair } => json_rpc_types::Response {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Ok(format!(
                "Successfully {} for attestation of the {} pair",
                request.method, asset_pair
            )),
            id: request.id.clone(),
        },
    }
}

#[derive(Clone, Debug)]
pub enum EventNotification {
    Announcement(AssetPair, OracleAnnouncement),
    Attestation(AssetPair, AttestationResponse),
}

#[derive(Clone, Serialize, Debug)]
#[serde(untagged)]
pub enum EventData {
    Announcement(OracleAnnouncement),
    Attestation(Option<AttestationResponse>),
}

pub struct ReceiverHandle(pub(crate) Receiver<EventNotification>);

impl Clone for ReceiverHandle {
    fn clone(&self) -> Self {
        Self(self.0.resubscribe())
    }
}

impl From<Receiver<EventNotification>> for ReceiverHandle {
    fn from(value: Receiver<EventNotification>) -> Self {
        Self(value)
    }
}

impl From<(AssetPair, OracleAttestation, Box<str>)> for EventNotification {
    fn from(value: (AssetPair, OracleAttestation, Box<str>)) -> Self {
        EventNotification::Attestation(
            value.0,
            AttestationResponse {
                event_id: value.2,
                signatures: value.1.signatures,
                values: value.1.outcomes,
            },
        )
    }
}

impl From<(AssetPair, OracleAnnouncement)> for EventNotification {
    fn from(value: (AssetPair, OracleAnnouncement)) -> Self {
        EventNotification::Announcement(value.0, value.1)
    }
}

#[derive(Serialize)]
pub struct EventBroadcastContent {
    channel: Box<str>,
    data: EventData,
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
