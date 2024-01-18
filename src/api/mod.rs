use actix_cors::Cors;
use dlc_messages::oracle_msgs::OracleAttestation;
use serde::{Deserialize, Serialize};

use crate::{
    config::{AssetPair, OracleSchedulerConfig},
    oracle::Oracle,
};
use secp256k1_zkp::schnorr::Signature;

use std::{collections::HashMap, sync::Arc};

use actix_web::{web, App, HttpServer, Result};

use self::ws::ReceiverHandle;

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

impl From<(Box<str>, OracleAttestation)> for AttestationResponse {
    fn from(value: (Box<str>, OracleAttestation)) -> Self {
        Self {
            event_id: value.0,
            signatures: value.1.signatures,
            values: value.1.outcomes,
        }
    }
}

type Context = web::Data<(
    Arc<HashMap<AssetPair, Arc<Oracle>>>,
    OracleSchedulerConfig,
    ReceiverHandle,
)>;

pub async fn run_api(
    data: (
        HashMap<AssetPair, Arc<Oracle>>,
        OracleSchedulerConfig,
        ReceiverHandle,
        bool,
    ),
    port: u16,
) -> anyhow::Result<()> {
    let (context, oracles_scheduler_config, rx, debug_mode) = data;
    let context = Arc::new(context);
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
                context.clone(),
                oracles_scheduler_config.clone(),
                rx.clone(),
            )))
            .service(factory)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await?;

    info!("HTTP API is running on port {}", port);
    Ok(())
}
