use std::time::{Duration, Instant};

use actix::{fut::wrap_future, prelude::*};
use actix_web_actors::ws;
use bytestring::ByteString;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use serde::Serialize;
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
    /// The Oracle instance the websocket allow to interact with
    oracle: Oracle,
    /// The stream of attestation to broadcast
    event_rx: ReceiverHandle,
}

impl PythiaWebSocket {
    pub fn new(oracle: Oracle, event_rx: ReceiverHandle) -> Self {
        Self {
            hb: Instant::now(),
            oracle,
            event_rx,
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

/// Handler for `ws::Message`
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for PythiaWebSocket {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        // process websocket messages
        println!("WS: {msg:?}");
        match msg {
            Ok(ws::Message::Ping(msg)) => {
                self.hb = Instant::now();
                ctx.pong(&msg);
            }
            Ok(ws::Message::Pong(_)) => {
                self.hb = Instant::now();
            }
            Ok(ws::Message::Text(request)) => {
                let request: Result<AttestationJRpcRequest, serde_json::Error> =
                    from_str(request.as_ref());
                let request = match request {
                    Ok(serialized_request) => serialized_request,
                    Err(e) => {
                        info!("WS: received invalid JRPC request: {}", e);
                        return;
                    }
                };
                let Some(event_id) = request.params.clone() else {
                    info!("WS: notification JRPC requests are not supported");
                    ctx.text(ByteString::from(
                        to_string_pretty(&jrpc_build_response(&request, None))
                            .expect("JRPC Response can always be parsed"),
                    ));
                    return;
                };
                let oracle = self.oracle.clone();
                let future_state = async move {
                    let state = oracle.oracle_state(&event_id).await.unwrap();
                    if let Some((_, Some(attestation))) = state {
                        let attestation_response: EventNotification =
                            (oracle.asset_pair_info.asset_pair, attestation, event_id).into();
                        to_string_pretty(&jrpc_build_response(&request, Some(attestation_response)))
                            .expect("JRPC Response can always be parsed")
                    } else {
                        info!("WS: eventID of received JRPC Request is invalid");
                        to_string_pretty(&jrpc_build_response(&request, None))
                            .expect("JRPC Response can always be parsed")
                    }
                };

                // https://stackoverflow.com/questions/72068485/how-use-postgres-deadpool-postgres-with-websocket-actix-actix-web-actors
                let fut = wrap_future::<_, Self>(future_state);
                let fut = fut.map(|result, _actor, ctx| {
                    ctx.text(result.to_string());
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
        // broadcast attestation to websocket clients
        println!("WS: Broadcasting attestation !");
        ctx.text(
            to_string_pretty(&EventBroadcast::from(
                item.expect("attestation are rare enough it should not lag begind"),
            ))
            .expect("serializable response"),
        );
    }
}

type EventBroadcast = json_rpc_types::Request<EventBroadcastContent, &'static str>;

impl From<EventNotification> for EventBroadcast {
    fn from(value: EventNotification) -> Self {
        json_rpc_types::Request {
            jsonrpc: json_rpc_types::Version::V2,
            method: "broadcast",
            params: Some(value.into()),
            id: None,
        }
    }
}

type AttestationJRpcRequest = json_rpc_types::Request<Box<str>>;
type AttestationJRpcResponse =
    json_rpc_types::Response<EventBroadcastContent, Box<str>, &'static str>;

fn jrpc_build_response(
    request: &AttestationJRpcRequest,
    attestation_response: Option<EventNotification>,
) -> AttestationJRpcResponse {
    match (&request.params, attestation_response) {
        (Some(_), Some(event_response)) => AttestationJRpcResponse {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Ok(event_response.into()),
            id: request.id.clone(),
        },
        (_, _) => AttestationJRpcResponse {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Err(json_rpc_types::Error {
                code: json_rpc_types::ErrorCode::InvalidParams,
                message: "eventId not found",
                data: request.params.clone(),
            }),
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
    Attestation(AttestationResponse),
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
                EventData::Attestation(event_data),
            ),
        };

        Self {
            channel: channel_string.into_boxed_str(),
            data: event_data,
        }
    }
}
