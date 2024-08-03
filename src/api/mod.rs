use actix_cors::Cors;
use actix_web::{web, App, HttpServer, Result};
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use secp256k1_zkp::schnorr::Signature;
use serde::{Deserialize, Serialize};

pub(crate) mod error;
use error::PythiaApiError;

mod http;
mod ws;

use crate::{config::AssetPair, contexts::api_context::ApiContext};

#[derive(PartialEq, Deserialize, Clone)]
struct EventChannel {
    #[serde(rename = "assetPair")]
    asset_pair: AssetPair,
    #[serde(rename = "type")]
    ty: EventType,
}
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GetRequest {
    #[serde(flatten)]
    asset_pair: EventChannel,
    event_id: Box<str>,
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
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

/// Build the actix-web App from the context and serve oracle events on the chosen port. It has scope "/v1".
/// It features an http server and a websocket that allow to subscribe to new announcements or attestations of an asset-pair.
pub async fn run_api_v1(
    context: ApiContext,
    port: u16,
    debug_mode: bool,
) -> Result<(), PythiaApiError> {
    HttpServer::new(move || {
        let mut factory = web::scope("/v1")
            // .service(announcements)
            .service(http::oracle_event_service)
            .service(http::config)
            .service(http::pub_key)
            .service(http::asset_return)
            .service(ws::websocket);
        if debug_mode {
            factory = factory.service(http::force)
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

impl From<(Box<str>, OracleAttestation)> for AttestationResponse {
    fn from(value: (Box<str>, OracleAttestation)) -> Self {
        Self {
            event_id: value.0,
            signatures: value.1.signatures,
            values: value.1.outcomes,
        }
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
