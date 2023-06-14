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
