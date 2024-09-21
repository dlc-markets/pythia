use actix_web::{get, post, web, Error, HttpResponse, Result};
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
pub(super) async fn pub_key(
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

    (!oracle.is_empty().await)
        .then_some(())
        .ok_or::<Error>(PythiaApiError::OracleEmpty.into())?;

    let event_id = (oracle.asset_pair_info.asset_pair.to_string()
        + &timestamp.timestamp().to_string())
        .into_boxed_str();
    let (announcement, maybe_attestation) = oracle
        .oracle_state(&event_id)
        .await
        .map_err(|e| PythiaApiError::OracleFail(e))?
        .ok_or::<Error>(PythiaApiError::OracleEventNotFoundError(timestamp.to_rfc3339()).into())?;

    match event_type {
        EventType::Announcement => Ok(HttpResponse::Ok().json(announcement)),
        EventType::Attestation => {
            let attestation = match maybe_attestation {
                Some(attestation) => Ok(attestation),
                None => {
                    if timestamp < Utc::now() {
                        Ok(oracle
                            .try_attest_event(&event_id)
                            .await
                            .map_err(|e| PythiaApiError::OracleFail(e))?
                            .expect("We checked Announcement exists and the oracle attested successfully so attestation exists now")
                        )
                    } else {
                        Err(actix_web::error::ErrorBadRequest(
                            "Oracle cannot sign a value not yet known, retry after ".to_string()
                                + &timestamp.to_rfc3339(),
                        ))
                    }
                }
            }?;

            let attestation_response = AttestationResponse {
                event_id,
                signatures: attestation.signatures,
                values: attestation.outcomes,
            };
            Ok(HttpResponse::Ok().json(attestation_response))
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct BatchAnnouncements {
    times: Vec<Box<str>>,
}

#[post("/asset/{asset_pair}/announcements")]
pub(super) async fn oracle_batch_announcements_service(
    context: ApiContext,
    path: web::Path<AssetPair>,
    data: web::Json<BatchAnnouncements>,
) -> Result<HttpResponse> {
    let asset_pair = path.into_inner();
    info!("POST /asset/{asset_pair}/announcements: {:#?}", data);

    let timestamps = data
        .times
        .iter()
        .map(|ts| Ok(DateTime::parse_from_rfc3339(ts).map_err(PythiaApiError::DatetimeParsing)?))
        .collect::<Result<Vec<_>>>()?;

    let oracle = context
        .oracles
        .get(&asset_pair)
        .ok_or(PythiaApiError::UnrecordedAssetPair(asset_pair))?;

    if oracle.is_empty().await {
        info!("no oracle events found");
        return Err(PythiaApiError::OracleEventNotFoundError(
            "Oracle did not announce anything".to_string(),
        )
        .into());
    }

    let events_ids = timestamps
        .into_iter()
        .map(|ts| (oracle.asset_pair_info.asset_pair.to_string() + &ts.timestamp().to_string()))
        .collect::<Vec<_>>();

    (!oracle.is_empty().await)
        .then_some(())
        .ok_or::<Error>(PythiaApiError::OracleEmpty.into())?;

    let announcements = oracle
        .oracle_many_announcements(events_ids)
        .await
        .map_err(|e| PythiaApiError::OracleFail(e))?;

    Ok(HttpResponse::Ok().json(announcements))
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
