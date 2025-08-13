use actix_web::{web, Error, HttpResponse, Result};
use chrono::{DateTime, FixedOffset, Utc};
use hex::ToHex;
use serde::{Deserialize, Serialize};

use crate::{
    api::{error::PythiaApiError, AttestationResponse, EventType},
    config::ConfigResponse,
    data_models::{asset_pair::AssetPair, event_ids::EventId, oracle_msgs::Announcement},
    schedule_context::{api_context::ApiContext, OracleContext},
};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct AssetPairFilters {
    asset_pair: AssetPair,
}

#[derive(Debug, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiOraclePubKey {
    public_key: String,
}

// #[derive(Debug, Serialize)]
// struct ApiOracleEvent {
//     asset_pair: AssetPair,
//     announcement: String,
//     attestation: Option<String>,
//     maturation: String,
//     outcome: Option<u64>,
// }

// https://github.com/actix/actix-web/issues/2866 explains why we commented this:
// #[get("/oracle/publickey")]
/// Get the public key of the oracle for the given asset pair
/// with request: `GET /oracle/publickey`
pub(super) async fn pub_key<Context: OracleContext>(
    context: ApiContext<Context>,
    filters: web::Query<AssetPairFilters>,
) -> Result<HttpResponse> {
    info!("GET /oracle/publickey");
    let oracle = match context.get_oracle(&filters.asset_pair) {
        None => return Err(PythiaApiError::UnrecordedAssetPair(filters.asset_pair).into()),
        Some(val) => val,
    };
    let res = ApiOraclePubKey {
        public_key: oracle.get_public_key().serialize().encode_hex::<String>(),
    };
    Ok(HttpResponse::Ok().json(res))
}

// https://github.com/actix/actix-web/issues/2866 explains why we commented this:
// #[get("/assets")]
/// Get the list of asset pairs supported by the oracle
/// with request: `GET /assets`
pub(super) async fn asset_return<Context: OracleContext>(
    context: ApiContext<Context>,
) -> Result<HttpResponse> {
    info!("GET /assets");
    Ok(HttpResponse::Ok().json(context.asset_pairs().collect::<Box<[_]>>()))
}

// https://github.com/actix/actix-web/issues/2866 explains why we commented this:
// #[get("/asset/{asset_id}/config")]
/// Get the configuration of the oracle for the given asset pair
/// with request: `GET /asset/{asset_id}/config`
pub(super) async fn config<Context: OracleContext>(
    context: ApiContext<Context>,
    path: web::Path<AssetPair>,
) -> Result<HttpResponse> {
    let asset_pair = path.into_inner();
    info!("GET /asset/{asset_pair}/config");
    let oracle = context
        .get_oracle(&asset_pair)
        .expect("We have this asset pair in our data");
    Ok(HttpResponse::Ok().json(ConfigResponse {
        pricefeed: &oracle.asset_pair_info.pricefeed.to_string(),
        announcement_offset: context.offset_duration,
        schedule: context.schedule(),
    }))
}

// https://github.com/actix/actix-web/issues/2866 explains why we commented this:
// #[get("/asset/{asset_pair}/{event_type}/{rfc3339_time}")]
/// Get the announcement/attestation of the oracle for the given asset pair and timestamp
/// with request: `GET /asset/{asset_pair}/{event_type}/{rfc3339_time}`
pub(super) async fn oracle_event_service<Context: OracleContext>(
    context: ApiContext<Context>,
    path: web::Path<(AssetPair, EventType, DateTime<FixedOffset>)>,
) -> Result<HttpResponse> {
    let (asset_pair, event_type, timestamp) = path.into_inner();
    info!("GET /asset/{asset_pair}/{event_type:?}/{timestamp}");

    let oracle = match context.get_oracle(&asset_pair) {
        None => return Err(PythiaApiError::UnrecordedAssetPair(asset_pair).into()),
        Some(val) => val,
    };

    (!oracle
        .is_empty()
        .await
        .map_err(PythiaApiError::OracleFail)?)
    .then_some(())
    .ok_or::<Error>(PythiaApiError::OracleEmpty.into())?;

    let event_id = EventId::spot_from_pair_and_timestamp(
        oracle.asset_pair_info.asset_pair,
        timestamp.with_timezone(&Utc),
    );
    let (announcement, maybe_attestation) = oracle
        .oracle_state(event_id)
        .await
        .map_err(PythiaApiError::OracleFail)?
        .ok_or::<Error>(PythiaApiError::OracleEventNotFoundError(timestamp.to_rfc3339()).into())?;

    match event_type {
        EventType::Announcement => Ok(HttpResponse::Ok().json(announcement)),
        EventType::Attestation => {
            let attestation = match maybe_attestation {
                Some(attestation) => Ok(attestation),
                None => {
                    if timestamp < Utc::now() {
                        Ok(oracle
                            .try_attest_event(event_id)
                            .await
                            .map_err(PythiaApiError::OracleFail)?
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

#[derive(Debug, Default, Deserialize)]
#[cfg_attr(test, derive(Serialize))]
#[serde(rename_all = "camelCase")]
pub(super) struct BatchAnnouncementsRequest {
    pub(super) maturities: Vec<DateTime<FixedOffset>>,
}

// https://github.com/actix/actix-web/issues/2866 explains why we commented this:
// #[post("/asset/{asset_pair}/announcements/batch")]
/// Gets the announcements from the oracle for all timestamps of the given asset pair
/// with request: `POST /asset/{asset_pair}/announcements/batch`
/// Body must be a JSON with a field `maturities` that is an array of timestamps
/// The response is a JSON array of announcements sorted by timestamp
pub(super) async fn oracle_batch_announcements_service<Context: OracleContext>(
    context: ApiContext<Context>,
    path: web::Path<AssetPair>,
    data: web::Json<BatchAnnouncementsRequest>,
) -> Result<HttpResponse> {
    let asset_pair = path.into_inner();
    info!("POST /asset/{asset_pair}/announcements/batch: {data:#?}");

    let oracle = context
        .get_oracle(&asset_pair)
        .ok_or(PythiaApiError::UnrecordedAssetPair(asset_pair))?;

    if oracle
        .is_empty()
        .await
        .map_err(PythiaApiError::OracleFail)?
    {
        info!("no oracle events found");
        return Err(PythiaApiError::OracleEventNotFoundError(
            "Oracle did not announce anything".to_string(),
        )
        .into());
    }

    let events_ids = data
        .0
        .maturities
        .iter()
        .map(|ts| {
            EventId::spot_from_pair_and_timestamp(
                oracle.asset_pair_info.asset_pair,
                ts.with_timezone(&Utc),
            )
        })
        .collect::<Vec<_>>();

    (!oracle
        .is_empty()
        .await
        .map_err(PythiaApiError::OracleFail)?)
    .then_some(())
    .ok_or::<Error>(PythiaApiError::OracleEmpty.into())?;

    let announcements = oracle
        .oracle_many_announcements(events_ids)
        .await
        .map_err(PythiaApiError::OracleFail)?;

    Ok(HttpResponse::Ok().json(announcements))
}

#[derive(Serialize, Deserialize, Debug)]
pub(super) struct ForceData {
    maturation: String,
    price: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ForceResponse {
    announcement: Announcement,
    attestation: AttestationResponse,
}

// https://github.com/actix/actix-web/issues/2866 explains why we commented this:
// #[post("/force")]
/// Forces the oracle to attest a new value for the given asset pair
/// with request: `POST /force`
/// Body must be a JSON with the following fields:
/// - `maturation`: the timestamp of the announcement
/// - `price`: the price for the attestation
///
/// The response is a JSON with the following fields:
/// - `announcement`: the produced announcement from the oracle
/// - `attestation`: the produced attestation from the oracle
///
/// The response is also sent to currently connected WebSocket clients
pub(super) async fn force<Context: OracleContext>(
    data: web::Json<ForceData>,
    context: ApiContext<Context>,
) -> Result<HttpResponse> {
    info!("POST /force");
    let ForceData { maturation, price } = data.0;

    let timestamp =
        DateTime::parse_from_rfc3339(&maturation).map_err(PythiaApiError::DatetimeParsing)?;

    let oracle = match context.get_oracle(&AssetPair::BtcUsd) {
        None => return Err(PythiaApiError::UnrecordedAssetPair(AssetPair::BtcUsd).into()),
        Some(val) => val,
    };

    let (announcement, attestation) = oracle
        .force_new_attest_with_price(timestamp.with_timezone(&Utc), price)
        .await
        .map_err(PythiaApiError::OracleFail)?;

    // Send event notification through the broadcast channel
    let _ = context
        .channel_sender
        .send((oracle.asset_pair_info.asset_pair, attestation.clone()).into());

    Ok(HttpResponse::Ok().json(ForceResponse {
        announcement: announcement.clone(),
        attestation: AttestationResponse {
            event_id: announcement.oracle_event.event_id,
            signatures: attestation.signatures,
            values: attestation.outcomes,
        },
    }))
}
