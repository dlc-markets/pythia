use crate::{AssetPairInfo, OracleConfig};

use dlc::secp_utils::schnorrsig_sign_with_nonce;
use dlc_messages::oracle_msgs::{
    EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent, Writeable,
};
use secp256k1_zkp::{
    hashes::sha256,
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
    db: DBconnection,
    secp: Secp256k1<All>,
    pricefeed: Box<dyn PriceFeed + Send + Sync>,
}

#[derive(Clone)]
pub struct Oracle {
    pub oracle_config: OracleConfig,
    asset_pair_info: AssetPairInfo,
    app_state: Arc<AppState>,
    keypair: KeyPair,
}

impl Oracle {
    pub fn new(
        oracle_config: OracleConfig,
        asset_pair_info: AssetPairInfo,
        secp: Secp256k1<All>,
        db: DBconnection,
        pricefeed: Box<dyn PriceFeed + Send + Sync>,
        keypair: KeyPair,
    ) -> Result<Oracle> {
        if !oracle_config.announcement_offset.is_positive() {
            return Err(OracleError::InvalidAnnouncementTimeError(
                oracle_config.announcement_offset,
            ));
        }

        // setup event database
        // let path = format!("events/{}", asset_pair_info.asset_pair);
        // info!("creating sled at {}", path);
        // let event_database = sled::open(path)?;

        Ok(Oracle {
            oracle_config,
            asset_pair_info,
            app_state: Arc::new(AppState {
                db,
                secp,
                pricefeed,
            }),
            keypair,
        })
    }
    pub fn get_public_key(self: &Oracle) -> SchnorrPublicKey {
        self.keypair.public_key().into()
    }
    pub async fn is_empty(&self) -> bool {
        self.app_state.db.is_empty().await
    }

    pub async fn create_announcement(
        &self,
        maturation: OffsetDateTime,
    ) -> Result<OracleAnnouncement> {
        let EventDescriptor::DigitDecompositionEvent(event) = &self.asset_pair_info.event_descriptor else {panic!("Error in db")};
        let digits = event.nb_digits;
        let mut sk_nonces = Vec::with_capacity(digits.into());
        let mut nonces = Vec::with_capacity(digits.into());
        {
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
        }

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

    pub async fn try_attest_event(&self, event_id: String) -> Result<Option<OracleAttestation>> {
        let Some(event) = self.app_state.db.get_event(&event_id).await? else {return Ok(None)};
        let ScalarsRecords::DigitsSkNonce(outstanding_sk_nonces) = event.scalars_records else {return Err(OracleError::AlreadyAttestatedError(event_id))};
        info!("retrieving pricefeeds for attestation");
        let outcome: u32 = self
            .app_state
            .pricefeed
            .retrieve_price(self.asset_pair_info.asset_pair, event.maturity)
            .await?
            .round() as u32;

        let outcomes = to_digit_decomposition_vec(outcome, event.digits);
        let (outcome_vec, signatures): (Vec<String>, Vec<Signature>) = outcomes
            .iter()
            .zip(outstanding_sk_nonces.iter())
            .map(|(outcome, outstanding_sk_nonce)| {
                (
                    outcome.to_owned(),
                    schnorrsig_sign_with_nonce(
                        &self.app_state.secp,
                        &Message::from_hashed_data::<sha256::Hash>(outcome.as_bytes()),
                        &self.keypair,
                        outstanding_sk_nonce,
                    ),
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
                        outcomes: to_digit_decomposition_vec(outcome, event.digits),
                    }),
                )))
            }
        }
    }
}
