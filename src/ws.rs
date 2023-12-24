use std::time::{Duration, Instant};

use actix::prelude::*;
use actix_web_actors::ws;
use dlc_messages::oracle_msgs::OracleAttestation;
use serde_json::to_string_pretty;

use crate::{api::AttestationResponse, oracle::Oracle};

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
}

impl PythiaWebSocket {
    pub fn new(oracle: Oracle) -> Self {
        Self {
            hb: Instant::now(),
            oracle,
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

    fn send_attestation(
        &self,
        attestation: OracleAttestation,
        event_id: &str,
        ctx: &mut <Self as Actor>::Context,
    ) {
        let attestation_response: AttestationResponse = (attestation, event_id).into();
        ctx.text(to_string_pretty(&attestation_response).expect("serializable response"))
    }
}

impl Actor for PythiaWebSocket {
    type Context = ws::WebsocketContext<Self>;

    /// Method is called on actor start. We start the heartbeat process here.
    fn started(&mut self, ctx: &mut Self::Context) {
        self.hb(ctx);
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
            Ok(ws::Message::Text(event_id)) => {
                let future_state = async {
                    let state = self.oracle.oracle_state(event_id.into()).await.unwrap();
                    if let Some((_, Some(attestation))) = state {
                        let attestation_response: AttestationResponse =
                            (attestation, event_id.as_ref()).into();
                        ctx.text(
                            to_string_pretty(&attestation_response).expect("serializable response"),
                        )
                    } else {
                        ctx.text("No attestation found with this eventId")
                    };
                };

                future_state.into_actor(self).spawn(ctx);
            }
            Ok(ws::Message::Binary(bin)) => (),
            Ok(ws::Message::Close(reason)) => {
                ctx.close(reason);
                ctx.stop();
            }
            _ => ctx.stop(),
        }
    }
}
