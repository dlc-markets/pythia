use actix_cors::Cors;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::Receiver;

use crate::{
    config::{AssetPair, OracleSchedulerConfig},
    oracle::Oracle,
};
use secp256k1_zkp::schnorr::Signature;

use std::collections::HashMap;

use actix_web::{web, App, HttpServer, Result};

use self::error::PythiaApiError;

pub(crate) mod error;
mod http;
pub(crate) mod ws;

#[derive(PartialEq, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EventChannel {
    asset_pair: AssetPair,
    #[serde(rename = "type")]
    ty: EventType,
}
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GetRequest {
    #[serde(flatten)]
    asset_pair: EventChannel,
    event_id: Box<str>,
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EventType {
    Announcement,
    Attestation,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AttestationResponse {
    pub(crate) event_id: Box<str>,
    pub(crate) signatures: Vec<Signature>,
    pub(crate) values: Vec<String>,
}
#[derive(Clone, Debug)]
pub enum EventNotification {
    Announcement(AssetPair, OracleAnnouncement),
    Attestation(AssetPair, AttestationResponse),
}

pub struct ReceiverHandle(pub(crate) Receiver<EventNotification>);

type Context = web::Data<(
    &'static HashMap<AssetPair, Oracle<'static>>,
    &'static OracleSchedulerConfig,
    ReceiverHandle,
)>;

pub(super) async fn run_api<'a>(
    data: (
        &'static HashMap<AssetPair, Oracle<'static>>,
        &'static OracleSchedulerConfig,
        ReceiverHandle,
        bool,
    ),
    port: u16,
) -> Result<(), PythiaApiError> {
    let (context, oracles_scheduler_config, rx, debug_mode) = data;
    HttpServer::new(move || {
        let mut factory = web::scope("/v1")
            // .service(announcements)
            .service(http::oracle_event_service)
            .service(http::config)
            .service(http::pubkey)
            .service(http::asset_return)
            .service(ws::websocket);
        if debug_mode {
            factory = factory.service(http::force)
        }

        App::new()
            .wrap(Cors::permissive())
            .app_data(web::Data::new((
                context,
                oracles_scheduler_config,
                rx.clone(),
            )))
            .service(factory)
    })
    .bind(("0.0.0.0", port))
    .map_err(PythiaApiError::SocketUnavailable)?
    .run()
    .await?;

    info!("HTTP API is running on port {}", port);
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
