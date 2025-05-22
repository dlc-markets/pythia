use actix_cors::Cors;
use actix_web::{web, App, HttpServer, Result};
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use secp256k1_zkp::schnorr::Signature;
use serde::{Deserialize, Serialize};

pub(crate) mod error;
use error::PythiaApiError;

mod http;
#[cfg(test)]
mod test;
mod ws;

use crate::{
    config::AssetPair,
    schedule_context::{api_context::ApiContext, OracleContext},
};

#[derive(PartialEq, Deserialize, Serialize, Clone, Copy)]
struct EventChannel {
    #[serde(rename = "assetPair")]
    asset_pair: AssetPair,
    #[serde(rename = "type")]
    ty: EventType,
}
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GetRequest {
    #[serde(flatten)]
    asset_pair: EventChannel,
    event_id: Box<str>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum EventType {
    Announcement,
    Attestation,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttestationResponse {
    event_id: Box<str>,
    signatures: Vec<Signature>,
    values: Vec<String>,
}
#[derive(Clone, Debug)]
pub(crate) enum EventNotification {
    Announcement(AssetPair, OracleAnnouncement),
    Attestation(AssetPair, AttestationResponse),
}

/// Builds the actix-web App from the context and serves oracle events on the chosen port. It has scope "/v1".
/// It features an HTTP server and a WebSocket that allows subscribing to new announcements or attestations of an asset pair.
///
/// Note that the `Context` is required to be `'static`, `Clone` and `Send`.
/// Arc or leak it to satisfy the bounds without cloning the `Context`.
pub async fn run_api_v1<Context>(
    context: ApiContext<Context>,
    port: u16,
    debug_mode: bool,
) -> Result<(), PythiaApiError>
where
    Context: OracleContext + Clone + Send + Unpin + 'static,
{
    HttpServer::new(move || {
        let mut factory = web::scope("/v1")
            // .service(announcements)
            .route(
                "/asset/{asset_pair}/{event_type}/{rfc3339_time}",
                web::get().to(http::oracle_event_service::<Context>),
            )
            .route(
                "/asset/{asset_id}/config",
                web::get().to(http::config::<Context>),
            )
            .route("/oracle/publickey", web::get().to(http::pub_key::<Context>))
            .route("/assets", web::get().to(http::asset_return::<Context>))
            .route(
                "/asset/{asset_pair}/announcements/batch",
                web::post().to(http::oracle_batch_announcements_service::<Context>),
            )
            // .route("/ws", web::get().to(ws::websocket::<Context>))
            .route("/ws", web::get().to(ws::websocket::<Context>));
        if debug_mode {
            factory = factory.route("/force", web::post().to(http::force::<Context>));
        }

        App::new()
            .wrap(Cors::permissive())
            .app_data(context.clone())
            .service(factory)
    })
    .bind(("0.0.0.0", port))
    .map_err(PythiaApiError::SocketUnavailable)?
    .run()
    .await?;
    Ok(())
}

impl From<OracleAttestation> for AttestationResponse {
    fn from(value: OracleAttestation) -> Self {
        Self {
            event_id: value.event_id.into_boxed_str(),
            signatures: value.signatures,
            values: value.outcomes,
        }
    }
}

impl From<(AssetPair, OracleAttestation)> for EventNotification {
    fn from(value: (AssetPair, OracleAttestation)) -> Self {
        EventNotification::Attestation(
            value.0,
            AttestationResponse {
                event_id: value.1.event_id.into_boxed_str(),
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
