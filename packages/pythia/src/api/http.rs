use actix_web::{web, Error, HttpResponse, Result};
use chrono::{DateTime, FixedOffset, Utc};
use futures::stream::StreamExt;
use futures_buffered::FuturesUnorderedBounded;
use hex::ToHex;
use serde::{Deserialize, Serialize};

use crate::{
    api::{error::PythiaApiError, AttestationResponse, EventType, Expiry},
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

    let event_ids = oracle
        .asset_pair_info
        .pricefeed
        .events_at_date(asset_pair, timestamp.with_timezone(&Utc));

    if event_ids.is_empty() {
        return Err(PythiaApiError::OracleEventNotFoundError(format!("at {timestamp}")).into());
    }

    match event_type {
        EventType::Announcement => {
            let mut announcements = oracle
                .oracle_many_announcements(event_ids)
                .await
                .map_err(PythiaApiError::OracleFail)?;

            // Sort by expiry for easier reading. Notice that a spot event will be considered first
            announcements.sort_unstable_by_key(|a| Expiry::try_from(a.oracle_event.event_id).ok());

            Ok(HttpResponse::Ok().json(announcements))
        }
        EventType::Attestation => {
            let attestations_status = event_ids
                .into_iter()
                .map(async |id| {
                    Ok(oracle
                        .oracle_state(id)
                        .await
                        .map_err(PythiaApiError::OracleFail)?
                        .ok_or(PythiaApiError::OracleEventNotFoundError(
                            timestamp.to_rfc3339(),
                        ))?
                        .1)
                })
                .collect::<FuturesUnorderedBounded<_>>()
                .collect::<Vec<_>>()
                .await;

            let mut error_buffer: Option<PythiaApiError> = None;

            let mut attestation_responses = attestations_status
                .into_iter()
                .filter_map(|r| {
                    r.map_err(|e| error_buffer.insert(e))
                        .ok()
                        .flatten()
                        .map(AttestationResponse::from)
                })
                .collect::<Vec<_>>();

            if !attestation_responses.is_empty() {
                attestation_responses.sort_unstable_by_key(|a| Expiry::try_from(a.event_id).ok());
                return Ok(HttpResponse::Ok().json(attestation_responses));
            } else if let Some(error) = error_buffer {
                return Err(error.into());
            }

            if timestamp < Utc::now() {
                let retrieved = oracle
                    .attest_at_date(timestamp.with_timezone(&Utc))
                    .await
                    .map_err(PythiaApiError::OracleFail)?;

                let mut responses = retrieved
                    .into_iter()
                    .filter_map(|attestation| {
                        Some(AttestationResponse::from(
                            attestation
                                .map_err(|e| error_buffer.insert(PythiaApiError::OracleFail(e)))
                                .ok()?,
                        ))
                    })
                    .collect::<Vec<_>>();

                if !responses.is_empty() {
                    responses.sort_unstable_by_key(|a| Expiry::try_from(a.event_id).ok());
                    Ok(HttpResponse::Ok().json(responses))
                } else if let Some(error) = error_buffer {
                    Err(error.into())
                } else {
                    Err(PythiaApiError::OracleEventNotFoundError(timestamp.to_rfc3339()).into())
                }
            } else {
                Err(actix_web::error::ErrorBadRequest(
                    "Oracle cannot sign a value not yet known, retry after ".to_string()
                        + &timestamp.to_rfc3339(),
                ))
            }
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
        .map(|ts| EventId::spot_from_pair_and_timestamp(asset_pair, ts.with_timezone(&Utc)))
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

// https://github.com/actix/actix-web/issues/2866 explains why we commented this:
// #[post("/forward/{asset_pair}/{expiry}/announcements/batch")]
/// Gets the announcements from the oracle for all timestamps of the given asset pair
/// with request: `POST /forward/{asset_pair}/{expiry}/announcements/batch`
/// Body must be a JSON with a field `maturities` that is an array of timestamps
/// The response is a JSON array of announcements sorted by timestamp
pub(super) async fn oracle_batch_forwards_service<Context: OracleContext>(
    context: ApiContext<Context>,
    path: web::Path<(AssetPair, Expiry)>,
    data: web::Json<BatchAnnouncementsRequest>,
) -> Result<HttpResponse> {
    let (asset_pair, expiry) = path.into_inner();
    info!("POST /forward/{asset_pair}/{expiry}/announcements/batch: {data:#?}");

    let Some(earliest_date) = data.0.maturities.iter().min() else {
        return Err(
            PythiaApiError::OracleEventNotFoundError("No maturities provided".to_string()).into(),
        );
    };

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

    let earliest_event_ids = oracle
        .asset_pair_info
        .pricefeed
        .events_at_date(asset_pair, earliest_date.with_timezone(&Utc));

    if !earliest_event_ids.into_iter().any(|id| {
        Expiry::try_from(id)
            .map(|e| e == expiry)
            .unwrap_or_default()
    }) {
        return Err(
            PythiaApiError::OracleEventNotFoundError(format!("with expiry {expiry}")).into(),
        );
    }

    let latest_date = data
        .0
        .maturities
        .iter()
        .max()
        .expect("We checked that there is at least one maturity");

    let expiry_as_datetime = DateTime::<Utc>::from(expiry);

    if latest_date.to_utc() >= expiry_as_datetime {
        return Err(PythiaApiError::TimestampGreaterThanExpiry.into());
    }

    let events_ids = data
        .0
        .maturities
        .iter()
        .map(|maturity| {
            if expiry_as_datetime == maturity.with_timezone(&Utc) {
                EventId::delivery_of_expiry_with_pair(asset_pair, expiry)
            } else {
                EventId::forward_of_expiry_with_pair_at_timestamp(
                    asset_pair,
                    expiry,
                    maturity.with_timezone(&Utc),
                )
            }
        })
        .collect::<Vec<_>>();

    let announcements = oracle
        .oracle_many_announcements(events_ids)
        .await
        .map_err(PythiaApiError::OracleFail)?;

    Ok(HttpResponse::Ok().json(announcements))
}
