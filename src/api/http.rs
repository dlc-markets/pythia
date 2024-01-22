use actix_web::{get, post, web, HttpResponse, Result};
use chrono::{DateTime, Utc};
use dlc_messages::oracle_msgs::OracleAnnouncement;
use hex::ToHex;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::{
    api::{error::PythiaApiError, AttestationResponse, EventType},
    config::{AssetPair, ConfigResponse},
    contexts::api_context::ApiContext,
};

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
            asset_pair: AssetPair::BtcUsd,
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

#[get("/oracle/publickey")]
pub(super) async fn pubkey(
    context: ApiContext,
    filters: web::Query<Filters>,
) -> Result<HttpResponse> {
    info!("GET /oracle/publickey");
    let oracle = match context.oracles.get(&filters.asset_pair) {
        None => return Err(PythiaApiError::UnrecordedAssetPair(filters.asset_pair).into()),
        Some(val) => val,
    };
    let res = ApiOraclePubKey {
        public_key: oracle.get_public_key().serialize().encode_hex::<String>(),
    };
    Ok(HttpResponse::Ok().json(res))
}

#[get("/assets")]
pub(super) async fn asset_return() -> Result<HttpResponse> {
    info!("GET /oracle/assets");
    Ok(HttpResponse::Ok().json(AssetPair::iter().collect::<Box<[_]>>()))
}

#[get("/asset/{asset_id}/config")]
pub(super) async fn config(
    context: ApiContext,
    path: web::Path<AssetPair>,
) -> Result<HttpResponse> {
    let asset_pair = path.into_inner();
    info!("GET /asset/{asset_pair}/config");
    let oracle = context
        .oracles
        .get(&asset_pair)
        .expect("We have this asset pair in our data");
    Ok(HttpResponse::Ok().json(ConfigResponse {
        pricefeed: oracle.asset_pair_info.pricefeed,
        announcement_offset: context.offset_duration,
        schedule: context.schedule.as_ref().clone(),
    }))
}

#[get("/asset/{asset_pair}/{event_type}/{rfc3339_time}")]
pub(super) async fn oracle_event_service(
    context: ApiContext,
    filters: web::Query<Filters>,
    path: web::Path<(AssetPair, EventType, String)>,
) -> Result<HttpResponse> {
    let (asset_pair, event_type, ts) = path.into_inner();
    info!(
        "GET /asset/{asset_pair}/{event_type:?}/{ts}: {:#?}",
        filters
    );
    let timestamp = DateTime::parse_from_rfc3339(&ts).map_err(PythiaApiError::DatetimeParsing)?;

    let oracle = match context.oracles.get(&asset_pair) {
        None => return Err(PythiaApiError::UnrecordedAssetPair(asset_pair).into()),
        Some(val) => val,
    };

    if oracle.is_empty().await {
        info!("no oracle events found");
        return Err(PythiaApiError::OracleEventNotFoundError(ts.to_string()).into());
    }

    let event_id = (oracle.asset_pair_info.asset_pair.to_string()
        + &timestamp.timestamp().to_string())
        .into_boxed_str();
    match oracle.oracle_state(&event_id).await {
        Err(error) => Err(PythiaApiError::OracleFail(error).into()),
        Ok(event_option) => match event_option {
            None => Err(PythiaApiError::OracleEventNotFoundError(timestamp.to_rfc3339()).into()),
            Some((announcement, maybe_attestation)) => match event_type {
                EventType::Announcement => Ok(HttpResponse::Ok().json(announcement)),
                EventType::Attestation => match maybe_attestation {
                    None => {
                        if timestamp < Utc::now() {
                            match oracle.try_attest_event(&event_id).await {
                                Err(error) => Err(PythiaApiError::OracleFail(error).into()),
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
                                    + &timestamp.to_rfc3339(),
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
pub(super) async fn force(data: web::Json<ForceData>, context: ApiContext) -> Result<HttpResponse> {
    info!("POST /force");
    let ForceData { maturation, price } = data.0;

    let timestamp =
        DateTime::parse_from_rfc3339(&maturation).map_err(PythiaApiError::DatetimeParsing)?;

    let oracle = match context.oracles.get(&AssetPair::BtcUsd) {
        None => return Err(PythiaApiError::UnrecordedAssetPair(AssetPair::BtcUsd).into()),
        Some(val) => val,
    };

    let (announcement, attestation) = oracle
        .force_new_attest_with_price(timestamp.with_timezone(&Utc), price)
        .await
        .map_err(PythiaApiError::OracleFail)?;
    Ok(HttpResponse::Ok().json(ForceResponse {
        announcement: announcement.clone(),
        attestation: AttestationResponse {
            event_id: announcement.oracle_event.event_id.into(),
            signatures: attestation.signatures,
            values: attestation.outcomes,
        },
    }))
}
