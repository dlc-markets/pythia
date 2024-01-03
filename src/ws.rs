use std::time::{Duration, Instant};

use actix::{fut::wrap_future, prelude::*};
use actix_web_actors::ws;
use bytestring::ByteString;
use serde_json::{from_str, to_string_pretty};
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};

use crate::{
    api::{AttestationResponse, ReceiverHandle},
    oracle::Oracle,
};

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
    attestation_rx: ReceiverHandle,
}

impl PythiaWebSocket {
    pub fn new(oracle: Oracle, attestation_rx: ReceiverHandle) -> Self {
        Self {
            hb: Instant::now(),
            oracle,
            attestation_rx,
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
        ctx.add_stream(BroadcastStream::from(self.attestation_rx.clone().0));
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
                    let state = oracle.oracle_state(event_id.clone().into()).await.unwrap();
                    if let Some((_, Some(attestation))) = state {
                        let attestation_response: AttestationResponse =
                            (&attestation, event_id.as_ref()).into();
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
impl StreamHandler<Result<AttestationResponse, BroadcastStreamRecvError>> for PythiaWebSocket {
    fn handle(
        &mut self,
        item: Result<AttestationResponse, BroadcastStreamRecvError>,
        ctx: &mut Self::Context,
    ) {
        // broadcast attestation to websocket clients
        println!("WS: Broadcasting attestation !");
        ctx.text(
            to_string_pretty(&AttestationBroadcast::from(
                item.expect("attestation are rare enough it should not lag begind"),
            ))
            .expect("serializable response"),
        );
    }
}

type AttestationBroadcast = json_rpc_types::Request<AttestationResponse, &'static str>;

impl From<AttestationResponse> for AttestationBroadcast {
    fn from(value: AttestationResponse) -> Self {
        json_rpc_types::Request {
            jsonrpc: json_rpc_types::Version::V2,
            method: "broadcast",
            params: Some(value),
            id: None,
        }
    }
}

type AttestationJRpcRequest = json_rpc_types::Request<Box<str>>;
type AttestationJRpcResponse =
    json_rpc_types::Response<AttestationResponse, Box<str>, &'static str>;

fn jrpc_build_response(
    request: &AttestationJRpcRequest,
    attestation_response: Option<AttestationResponse>,
) -> AttestationJRpcResponse {
    match (&request.params, attestation_response) {
        (Some(_), Some(attestation_response)) => AttestationJRpcResponse {
            jsonrpc: json_rpc_types::Version::V2,
            payload: Ok(attestation_response),
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
