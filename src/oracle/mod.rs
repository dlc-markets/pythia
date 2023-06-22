use crate::{oracle::crypto::sign_outcome, AssetPairInfo};

use dlc_messages::oracle_msgs::{
    EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent, Writeable,
};
use secp256k1_zkp::{
    rand::{thread_rng, RngCore},
    All, KeyPair, Message, Secp256k1, XOnlyPublicKey as SchnorrPublicKey,
};
use std::sync::Arc;
use time::OffsetDateTime;

use crate::pricefeeds::PriceFeed;

use secp256k1_zkp::schnorr::Signature;

mod error;
pub use error::OracleError;
pub use error::Result;

pub mod postgres;

use self::crypto::{to_digit_decomposition_vec, NoncePoint, OracleSignature, SigningScalar};
use self::postgres::*;

mod crypto;
struct AppState {
    db: DBconnection, // private to ensure that only the oracle's methods can interact with DB
    secp: Secp256k1<All>,
    pricefeed: Box<dyn PriceFeed + Send + Sync>, // private to ensure the oracle uses a unique consistent pricefeeder
}

/// A stateful digits event oracle application. It prepares announcements and try to attest them on demand. It also managed the storage of announcements and attestations.
#[derive(Clone)]
pub struct Oracle {
    /// Oracle attestation event format summary
    pub asset_pair_info: AssetPairInfo,
    app_state: Arc<AppState>, // private to ensure that only the oracle's methods can interact with DB
    keypair: KeyPair,         // MUST be private since it contains the oracle private key
}

impl Oracle {
    /// Create a new instance of oracle for a numerical outcome given an libsecp256k1 context, a postgres DB connection and a keypair.
    pub fn new(
        asset_pair_info: AssetPairInfo,
        secp: Secp256k1<All>,
        db: DBconnection,
        keypair: KeyPair,
    ) -> Result<Oracle> {
        let pricefeed = asset_pair_info.pricefeed.get_pricefeed();

        Ok(Oracle {
            asset_pair_info,
            app_state: Arc::new(AppState {
                db,
                secp,
                pricefeed,
            }),
            keypair,
        })
    }
    /// The oracle public key
    pub fn get_public_key(self: &Oracle) -> SchnorrPublicKey {
        self.keypair.public_key().into()
    }
    /// Check if the oracle announced at least one event
    pub async fn is_empty(&self) -> bool {
        self.app_state.db.is_empty().await
    }

    /// Create an oracle announcement that it will sign the price at givent maturity instant
    pub async fn create_announcement(
        &self,
        maturation: OffsetDateTime,
    ) -> Result<OracleAnnouncement> {
        let EventDescriptor::DigitDecompositionEvent(event) = &self.asset_pair_info.event_descriptor else {panic!("Error in db")};
        let digits = event.nb_digits;
        let mut sk_nonces = Vec::with_capacity(digits.into());
        let mut nonces = Vec::with_capacity(digits.into());
        {
            // Begin scope to emsure ThreadRng is drop at compile time so that Oraacle derive Send AutoTrait
            let mut rng = thread_rng();
            for _ in 0..digits {
                let mut sk_nonce = [0u8; 32];
                rng.fill_bytes(&mut sk_nonce);
                let oracle_r_kp =
                    secp256k1_zkp::KeyPair::from_seckey_slice(&self.app_state.secp, &sk_nonce)
                        .unwrap();
                let nonce = SchnorrPublicKey::from_keypair(&oracle_r_kp).0;
                sk_nonces.push(sk_nonce);
                nonces.push(nonce);
            }
        } // End scope: ThreadRng is drop at compile time so that Oracle derives Send AutoTrait

        let oracle_event = OracleEvent {
            oracle_nonces: nonces,
            event_maturity_epoch: maturation.unix_timestamp() as u32,
            event_descriptor: self.asset_pair_info.event_descriptor.clone(),
            event_id: self.asset_pair_info.asset_pair.to_string().to_lowercase()
                + maturation.unix_timestamp().to_string().as_str(),
        };

        let announcement = OracleAnnouncement {
            announcement_signature: self.app_state.secp.sign_schnorr(
                &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                    &oracle_event.encode(),
                ),
                &self.keypair,
            ),
            oracle_public_key: self.keypair.public_key().into(),
            oracle_event,
        };
        let _ = &self
            .app_state
            .db
            .insert_announcement(&announcement, sk_nonces)
            .await?;
        info!(
            "created oracle event (announcement only) with maturation {} and announcement {:#?}",
            maturation, &announcement
        );

        Ok(announcement)
    }

    /// Attest event with given eventID. Return None if it was not announced, a PriceFeeder error if it the outcome is not available.
    /// Store in DB and returm some oracle attestation if event is attested successfully.
    pub async fn try_attest_event(&self, event_id: String) -> Result<Option<OracleAttestation>> {
        let Some(event) = self.app_state.db.get_event(&event_id).await? else {return Ok(None)};
        let ScalarsRecords::DigitsSkNonce(outstanding_sk_nonces) = event.scalars_records else {return Err(OracleError::AlreadyAttestatedError(event_id))};
        info!("retrieving pricefeeds for attestation");
        let outcome = self
            .app_state
            .pricefeed
            .retrieve_price(self.asset_pair_info.asset_pair, event.maturity)
            .await?;

        let outcomes = to_digit_decomposition_vec(outcome, event.digits, event.precision);
        let (outcome_vec, signatures): (Vec<String>, Vec<Signature>) = outcomes
            .iter()
            .zip(outstanding_sk_nonces.iter())
            .map(|(outcome, outstanding_sk_nonce)| {
                sign_outcome(
                    &self.app_state.secp,
                    &self.keypair,
                    outcome,
                    outstanding_sk_nonce,
                )
            })
            .unzip();

        let attestation = OracleAttestation {
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes: outcome_vec,
        };
        let _ = &self
            .app_state
            .db
            .update_to_attestation(event_id.as_ref(), &attestation, outcome)
            .await?;
        Ok(Some(attestation))
    }

    /// If it exists, return an event announcement and attestation.
    pub async fn oracle_state(
        &self,
        event_id: String,
    ) -> Result<Option<(OracleAnnouncement, Option<OracleAttestation>)>> {
        let Some(event) = self.app_state.db.get_event(&event_id).await? else {return Ok(None)};
        let oracle_event = OracleEvent {
            oracle_nonces: event.nonce_public.clone(),
            event_maturity_epoch: event.maturity.unix_timestamp() as u32,
            event_descriptor: self.asset_pair_info.event_descriptor.clone(),
            event_id,
        };

        let announcement = OracleAnnouncement {
            announcement_signature: event.announcement_signature,
            oracle_public_key: self.keypair.public_key().into(),
            oracle_event,
        };
        match event.scalars_records {
            ScalarsRecords::DigitsSkNonce(_) => Ok(Some((announcement, None))),
            ScalarsRecords::DigitsAttestations(outcome, sigs) => {
                let full_signatures = sigs
                    .iter()
                    .zip(event.nonce_public)
                    .map(|(s, n)| {
                        <(NoncePoint, SigningScalar) as Into<OracleSignature>>::into((
                            NoncePoint(n),
                            SigningScalar(*s),
                        ))
                        .into()
                    })
                    .collect();
                Ok(Some((
                    announcement,
                    Some(OracleAttestation {
                        oracle_public_key: self.keypair.public_key().into(),
                        signatures: full_signatures,
                        outcomes: to_digit_decomposition_vec(
                            outcome,
                            event.digits,
                            event.precision,
                        ),
                    }),
                )))
            }
        }
    }
}

#[cfg(test)]
mod test {

    use std::{path::Path, sync::Arc, time::Duration};

    use actix_web;
    use dlc_messages::oracle_msgs::{
        DigitDecompositionEventDescriptor, EventDescriptor, OracleAttestation, Writeable,
    };
    use secp256k1_zkp::{rand, All, KeyPair, Message, Secp256k1};
    use serde::Serialize;
    use time::{macros::datetime, OffsetDateTime};

    use crate::{
        api::announcement,
        common::{AssetPair, AssetPairInfo},
        oracle::{postgres::DBconnection, Oracle},
        pricefeeds::{ImplementedPriceFeed, PriceFeed, PriceFeedError},
    };

    use crate::pricefeeds::ImplementedPriceFeed::Lnm;

    use sqlx_mock::TestPostgres;

    use super::OracleError;

    async fn setup_oracle(
        tbd: &TestPostgres,
        precision: i32,
        nb_digits: u16,
        pricefeed: ImplementedPriceFeed,
    ) -> Oracle {
        let asset_pair = AssetPair::BTCUSD;
        let event_descriptor =
            EventDescriptor::DigitDecompositionEvent(DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: false,
                unit: "btc/usd".into(),
                precision,
                nb_digits,
            });
        let asset_pair_info = AssetPairInfo {
            pricefeed,
            asset_pair,
            event_descriptor,
        };

        let secp = Secp256k1::new();
        let (secret_key, public_key) = secp.generate_keypair(&mut rand::thread_rng());
        let keypair = KeyPair::from_secret_key(&secp, &secret_key);
        let db = DBconnection(tbd.get_pool().await);
        return Oracle::new(asset_pair_info, secp, db, keypair).unwrap();
    }

    #[actix_web::test]
    async fn test_oracle_setup() {
        let tbd = TestPostgres::new(
            "postgres://postgres:postgres@127.0.0.1:5432".into(),
            Path::new("./migrations"),
        );
        let oracle = setup_oracle(&tbd, 0, 20, Lnm).await;
        let EventDescriptor::DigitDecompositionEvent(event) = oracle.asset_pair_info.clone().event_descriptor else {panic!("Invalid event type")};
        assert_eq!((0, 20), (event.precision, event.nb_digits));
        let oracle = setup_oracle(&tbd, 10, 20, Lnm).await;
        let EventDescriptor::DigitDecompositionEvent(event) = oracle.asset_pair_info.clone().event_descriptor else {panic!("Invalid event type")};
        assert_eq!((10, 20), (event.precision, event.nb_digits))
    }

    async fn test_announcement(oracle: &Oracle, date: OffsetDateTime) {
        let oracle_announcement = oracle.create_announcement(date).await.unwrap();
        assert_eq!(
            oracle_announcement.oracle_event.event_id,
            "btcusd".to_owned() + date.unix_timestamp().to_string().as_str()
        );

        let secp = &oracle.app_state.secp;
        secp.verify_schnorr(
            &oracle_announcement.announcement_signature,
            &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                oracle_announcement.oracle_event.encode().as_ref(),
            ),
            &oracle.get_public_key(),
        )
        .unwrap();
    }

    #[actix_web::test]
    async fn announcements_tests() {
        let tbd = TestPostgres::new(
            "postgres://postgres:postgres@127.0.0.1:5432".into(),
            Path::new("./migrations"),
        );
        let oracle = setup_oracle(&tbd, 12, 32, Lnm).await;
        let now = OffsetDateTime::now_utc();
        let dates = [60, 3600, 24 * 3600, 7 * 24 * 3600]
            .iter()
            .map(|t| now - Duration::new(*t, 0))
            .chain(
                [
                    datetime!(2022-01-01 0:00 +11),
                    datetime!(2023-01-01 0:00 +11),
                ]
                .into_iter(),
            );
        for date in dates {
            test_announcement(&oracle, date).await;
        }
    }

    async fn test_attestation(oracle: &Oracle, date: OffsetDateTime) {
        let oracle_announcement = oracle.create_announcement(date).await.unwrap();
        let now = OffsetDateTime::now_utc();
        let oracle_attestation = match oracle
            .try_attest_event(oracle_announcement.oracle_event.event_id)
            .await
        {
            Ok(attestation) => attestation,
            Err(OracleError::PriceFeedError(error)) => {
                let PriceFeedError::PriceNotAvailableError(_, date) = error else {panic!("Pricefeeder {:?} did not respond for this date {}", oracle.asset_pair_info.pricefeed, date)};
                assert!(date < now);
                return;
            }
            Err(OracleError::AlreadyAttestatedError(eventid)) => {
                let (_, ts_str) = eventid.split_at(6);
                let date =
                    OffsetDateTime::from_unix_timestamp(ts_str.parse::<i64>().unwrap()).unwrap();
                println!("Event already at date: {}", date);
                return;
            }
            _ => panic!("DB error"),
        };
        match oracle_attestation {
            None => assert!(
                oracle_announcement.oracle_event.event_maturity_epoch > now.unix_timestamp() as u32
            ),
            Some(attestation) => {
                let EventDescriptor::DigitDecompositionEvent(event) =
                    &oracle.asset_pair_info.event_descriptor else {panic!("Invalid event type")};
                let precision = event.precision;
                assert_eq!(
                    oracle
                        .app_state
                        .pricefeed
                        .retrieve_price(AssetPair::BTCUSD, date)
                        .await
                        .unwrap(),
                    attestation
                        .outcomes
                        .iter()
                        .rev()
                        .enumerate()
                        .map(|c| c.1.parse::<f64>().unwrap()
                            * (2_f64.powf(c.0 as f64 - (precision as f64))))
                        .sum::<f64>()
                );
                let secp = &oracle.app_state.secp;
                for (outcome, signature) in attestation.outcomes.iter().zip(attestation.signatures)
                {
                    secp.verify_schnorr(
                        &signature,
                        &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                            outcome.as_ref(),
                        ),
                        &oracle.get_public_key(),
                    )
                    .unwrap();
                }
            }
        }
    }

    #[actix_web::test]
    async fn attestations_test() {
        let tbd = TestPostgres::new(
            "postgres://postgres:postgres@127.0.0.1:5432".into(),
            Path::new("./migrations"),
        );
        let oracle = setup_oracle(&tbd, 12, 32, Lnm).await;
        let now = OffsetDateTime::now_utc().replace_second(0).unwrap();
        let dates = [60, 3600, 24 * 3600, 7 * 24 * 3600]
            .iter()
            .map(|t| now - Duration::new(*t, 0))
            .chain(
                [
                    datetime!(2022-01-01 0:00 +11),
                    datetime!(2023-01-01 0:00 +11),
                ]
                .into_iter(),
            );
        for date in dates {
            test_attestation(&oracle, date).await;
        }
    }
}
