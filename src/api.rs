use std::collections::HashMap;

use actix_cors::Cors;
use actix_web::{get, post, web, App, HttpRequest, HttpResponse, HttpServer, Result};
use actix_web_actors::ws;
use dlc_messages::oracle_msgs::OracleAnnouncement;
use hex::ToHex;
use secp256k1_zkp::schnorr::Signature;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    common::{AssetPair, ConfigResponse, OracleSchedulerConfig},
    error::PythiaError,
    oracle::Oracle,
    ws::{PythiaWebSocket, ReceiverHandle},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum SortOrder {
    Insertion,
    ReverseInsertion,
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Filters {
    sort_by: SortOrder,
    page: u32,
    asset_pair: AssetPair,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            sort_by: SortOrder::ReverseInsertion,
            page: 0,
            asset_pair: AssetPair::Btcusd,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiOraclePubKey {
    public_key: String,
}

#[derive(Debug, Serialize)]
struct ApiOracleEvent {
    asset_pair: AssetPair,
    announcement: String,
    attestation: Option<String>,
    maturation: String,
    outcome: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum EventType {
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

#[get("/oracle/publickey")]
async fn pubkey(
    oracles: web::Data<(HashMap<AssetPair, Oracle>, OracleSchedulerConfig)>,
    filters: web::Query<Filters>,
) -> Result<HttpResponse> {
    info!("GET /oracle/publickey");
    let oracle = match oracles.0.get(&filters.asset_pair) {
        None => return Err(PythiaError::UnrecordedAssetPairError(filters.asset_pair).into()),
        Some(val) => val,
    };
    let res = ApiOraclePubKey {
        public_key: oracle.get_public_key().serialize().encode_hex::<String>(),
    };
    Ok(HttpResponse::Ok().json(res))
}

#[get("/asset")]
async fn asset_return() -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json([AssetPair::Btcusd]))
}

#[get("/asset/{asset_id}/config")]
async fn config(
    oracles: web::Data<(HashMap<AssetPair, Oracle>, OracleSchedulerConfig)>,
    path: web::Path<AssetPair>,
) -> Result<HttpResponse> {
    let asset_pair = path.into_inner();
    info!("GET /asset/{asset_pair}/config");
    let oracle = oracles
        .0
        .get(&asset_pair)
        .expect("We have this asset pair in our data");
    Ok(HttpResponse::Ok().json(ConfigResponse::from((
        oracle.asset_pair_info.pricefeed,
        oracles.1,
    ))))
}

#[get("/asset/{asset_pair}/{event_type}/{rfc3339_time}")]
async fn oracle_event_service(
    oracles: web::Data<(HashMap<AssetPair, Oracle>, OracleSchedulerConfig)>,
    filters: web::Query<Filters>,
    path: web::Path<(AssetPair, EventType, String)>,
) -> Result<HttpResponse> {
    let (asset_pair, event_type, ts) = path.into_inner();
    info!(
        "GET /asset/{asset_pair}/{event_type:?}/{ts}: {:#?}",
        filters
    );
    let timestamp =
        OffsetDateTime::parse(&ts, &Rfc3339).map_err(PythiaError::DatetimeParseError)?;

    let oracle = match oracles.0.get(&asset_pair) {
        None => return Err(PythiaError::UnrecordedAssetPairError(asset_pair).into()),
        Some(val) => val,
    };

    if oracle.is_empty().await {
        info!("no oracle events found");
        return Err(PythiaError::OracleEventNotFoundError(ts.to_string()).into());
    }

    info!("retrieving oracle event with maturation {}", ts);
    let event_id =
        ("btcusd".to_string() + &timestamp.unix_timestamp().to_string()).into_boxed_str();
    match oracle.oracle_state(&event_id).await {
        Err(error) => Err(PythiaError::OracleError(error).into()),
        Ok(event_option) => match event_option {
            None => Err(PythiaError::OracleEventNotFoundError(
                timestamp.format(&Rfc3339).expect("Format is good"),
            )
            .into()),
            Some((announcement, maybe_attestation)) => match event_type {
                EventType::Announcement => Ok(HttpResponse::Ok().json(announcement)),
                EventType::Attestation => match maybe_attestation {
                    None => {
                        if timestamp < OffsetDateTime::now_utc() {
                            match oracle.try_attest_event(&event_id).await {
                                Err(error) => Err(PythiaError::OracleError(error).into()),
                                Ok(maybe_attestation) => {
                                    let attestation = maybe_attestation.expect("We checked Announcement exists and the oracle attested successfully so attestation exists now");
                                    let attestation_response = AttestationResponse {
                                        event_id: event_id.clone(),
                                        signatures: attestation.signatures,
                                        values: attestation.outcomes,
                                    };
                                    Ok(HttpResponse::Ok().json(attestation_response))
                                }
                            }
                        } else {
                            Err(actix_web::error::ErrorBadRequest(
                                "Oracle cannot sign a value not yet known, retry after "
                                    .to_string()
                                    + &timestamp.format(&Rfc3339).expect("Format is good"),
                            ))
                        }
                    }
                    Some(attestation) => {
                        let attestation_response = AttestationResponse {
                            event_id,
                            signatures: attestation.signatures,
                            values: attestation.outcomes,
                        };
                        Ok(HttpResponse::Ok().json(attestation_response))
                    }
                },
            },
        },
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct ForceData {
    maturation: String,
    price: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ForceResponse {
    announcement: OracleAnnouncement,
    attestation: AttestationResponse,
}

#[post("/force")]
async fn force(
    data: web::Json<ForceData>,
    oracles: web::Data<(HashMap<AssetPair, Oracle>, OracleSchedulerConfig)>,
) -> Result<HttpResponse> {
    info!("!!! Forced Request !!!");
    let ForceData { maturation, price } = data.0;

    let timestamp =
        OffsetDateTime::parse(&maturation, &Rfc3339).map_err(PythiaError::DatetimeParseError)?;

    let oracle = match oracles.0.get(&AssetPair::Btcusd) {
        None => return Err(PythiaError::UnrecordedAssetPairError(AssetPair::Btcusd).into()),
        Some(val) => val,
    };

    let (announcement, attestation) = oracle
        .force_new_attest_with_price(timestamp, price)
        .await
        .map_err(PythiaError::OracleError)?;
    Ok(HttpResponse::Ok().json(ForceResponse {
        announcement: announcement.clone(),
        attestation: AttestationResponse {
            event_id: announcement.oracle_event.event_id.into(),
            signatures: attestation.signatures,
            values: attestation.outcomes,
        },
    }))
}

pub async fn run_api(
    data: (
        HashMap<AssetPair, Oracle>,
        OracleSchedulerConfig,
        ReceiverHandle,
        bool,
    ),
    port: u16,
) -> anyhow::Result<()> {
    let (oracles, oracles_scheduler_config, rx, debug_mode) = data;
    info!("starting server");
    HttpServer::new(move || {
        let mut factory = web::scope("/v1")
            // .service(announcements)
            .service(oracle_event_service)
            .service(config)
            .service(pubkey)
            .service(asset_return)
            .service(index);
        if debug_mode {
            factory = factory.service(force)
        }
        App::new()
            .wrap(Cors::permissive())
            .app_data(web::Data::new((
                oracles.clone(),
                oracles_scheduler_config,
                rx.clone(),
            )))
            .service(factory)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await?;
    Ok(())
}

// async fn get_event_at_timestamp(
//     oracle: &Oracle,
//     ts: &OffsetDateTime,
// ) -> Result<Option<(OracleAnnouncement, Option<OracleAttestation>)>, OracleError> {
//     oracle
//         .oracle_state("btcusd".to_string() + &ts.unix_timestamp().to_string())
//         .await
// }

#[get("/ws")]
async fn index(
    oracles: web::Data<(
        HashMap<AssetPair, Oracle>,
        OracleSchedulerConfig,
        ReceiverHandle,
    )>,
    stream: web::Payload,
    req: HttpRequest,
) -> Result<HttpResponse> {
    let resp = ws::start(
        PythiaWebSocket::new(
            oracles
                .0
                .get(&AssetPair::Btcusd)
                .ok_or(PythiaError::UnrecordedAssetPairError(AssetPair::Btcusd))?
                .clone(),
            oracles.2.clone(),
        ),
        &req,
        stream,
    );
    println!("{:?}", resp);
    resp
}
