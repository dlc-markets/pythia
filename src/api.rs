use std::collections::HashMap;

use actix_web::{get, web, HttpResponse};
use hex::ToHex;
use secp256k1_zkp::schnorr::Signature;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{common::AssetPair, error::PythiaError, oracle::Oracle};

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
            asset_pair: AssetPair::BTCUSD,
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

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AttestationResponse {
    event_id: String,
    signatures: Vec<Signature>,
    values: Vec<String>,
}

#[get("/oracle/publickey")]
async fn pubkey(
    oracles: web::Data<HashMap<AssetPair, Oracle>>,
    filters: web::Query<Filters>,
) -> actix_web::Result<HttpResponse, actix_web::Error> {
    info!("GET /oracle/publickey");
    let oracle = match oracles.get(&filters.asset_pair) {
        None => return Err(PythiaError::UnrecordedAssetPairError(filters.asset_pair).into()),
        Some(val) => val,
    };
    let res = ApiOraclePubKey {
        public_key: oracle.get_public_key().serialize().encode_hex::<String>(),
    };
    Ok(HttpResponse::Ok().json(res))
}

#[get("/config")]
async fn config(
    oracles: web::Data<HashMap<AssetPair, Oracle>>,
) -> actix_web::Result<HttpResponse, actix_web::Error> {
    info!("GET /config");
    Ok(HttpResponse::Ok().json(
        oracles
            .values()
            .next()
            .expect("no asset pairs recorded")
            .oracle_config,
    ))
}

#[get("/asset/{asset_pair}/{event_type}/{rfc3339_time}")]
async fn announcement(
    oracles: web::Data<HashMap<AssetPair, Oracle>>,
    filters: web::Query<Filters>,
    path: web::Path<(AssetPair, EventType, String)>,
) -> actix_web::Result<HttpResponse, actix_web::Error> {
    let (asset_pair, event_type, ts) = path.into_inner();
    info!(
        "GET /asset/{asset_pair}/{event_type:?}/{ts}: {:#?}",
        filters
    );
    let timestamp =
        OffsetDateTime::parse(&ts, &Rfc3339).map_err(PythiaError::DatetimeParseError)?;

    let oracle = match oracles.get(&asset_pair) {
        None => return Err(PythiaError::UnrecordedAssetPairError(asset_pair).into()),
        Some(val) => val,
    };

    if oracle.is_empty().await {
        info!("no oracle events found");
        return Err(PythiaError::OracleEventNotFoundError(ts.to_string()).into());
    }

    info!("retrieving oracle event with maturation {}", ts);
    let event_id = "btcusd".to_string() + &timestamp.unix_timestamp().to_string();
    match oracle.oracle_state(event_id.clone()).await {
        Err(error) => Err(PythiaError::OracleError(error).into()),
        Ok(event_option) => match event_option {
            None => Err(PythiaError::OracleEventNotFoundError(event_id).into()),
            Some((announcement, maybe_attestation)) => match event_type {
                EventType::Announcement => Ok(HttpResponse::Ok().json(announcement)),
                EventType::Attestation => match maybe_attestation {
                    None => {
                        if timestamp < OffsetDateTime::now_utc() {
                            match oracle.try_attest_event(event_id.clone()).await {
                                Err(error) => Err(PythiaError::OracleError(error).into()),
                                Ok(maybe_attestation) => {
                                    let attestation = maybe_attestation.expect("We checked Announcement exists and the oracle attested successfully so attestation exist now");
                                    let attestation_response = AttestationResponse {
                                        event_id,
                                        signatures: attestation.signatures,
                                        values: attestation.outcomes,
                                    };
                                    Ok(HttpResponse::Ok().json(attestation_response))
                                }
                            }
                        } else {
                            Ok(HttpResponse::Ok().json(announcement))
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

// async fn get_event_at_timestamp(
//     oracle: &Oracle,
//     ts: &OffsetDateTime,
// ) -> Result<Option<(OracleAnnouncement, Option<OracleAttestation>)>, OracleError> {
//     oracle
//         .oracle_state("btcusd".to_string() + &ts.unix_timestamp().to_string())
//         .await
// }
