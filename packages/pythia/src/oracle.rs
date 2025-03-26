use crate::{config::AssetPair, oracle::crypto::sign_outcome, AssetPairInfo};

use chrono::{DateTime, Utc};
use dlc_messages::oracle_msgs::{
    EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent,
};
use futures::{stream, TryStreamExt};
use futures_buffered::BufferedTryStreamExt;
use lightning::util::ser::Writeable;
use secp256k1_zkp::{
    hashes::{sha256, Hash},
    rand::{rngs::ThreadRng, thread_rng, RngCore},
    schnorr::Signature,
    Keypair, Message, XOnlyPublicKey,
};

use crate::SECP;
type OracleAnnouncementWithSkNonces = (OracleAnnouncement, Vec<[u8; 32]>);

mod crypto;
pub(crate) mod error;
pub(crate) mod postgres;
#[cfg(test)]
mod test;

use crypto::{to_digit_decomposition_vec, NoncePoint, OracleSignature, SigningScalar};
use error::{OracleError, Result};
use postgres::*;
/// Number of maturations to process in each batch to prevent database connection pool exhaustion
/// and maintain optimal performance while processing backlogged announcements
pub const CHUNK_SIZE: usize = 200;

/// A stateful digits event oracle application. It prepares announcements and try to attest them on demand. It also managed the storage of announcements and attestations.
#[derive(Clone)]
pub struct Oracle {
    /// Oracle attestation event format summary
    pub asset_pair_info: AssetPairInfo,
    db: DBconnection,
    keypair: Keypair,
}

impl Oracle {
    /// Create a new instance of oracle for a numerical outcome given a postgres DB connection and a keypair.
    pub fn new(asset_pair_info: AssetPairInfo, db: DBconnection, keypair: Keypair) -> Oracle {
        Oracle {
            asset_pair_info,
            db,
            keypair,
        }
    }
    /// The oracle public key
    pub fn get_public_key(&self) -> XOnlyPublicKey {
        self.keypair.public_key().into()
    }
    /// Check if the oracle announced at least one event
    pub async fn is_empty(&self) -> bool {
        self.db.is_empty().await
    }

    fn prepare_announcement(
        &self,
        maturation: DateTime<Utc>,
        rng: &mut ThreadRng,
    ) -> Result<OracleAnnouncementWithSkNonces> {
        let event_id = self.asset_pair_info.asset_pair.to_string().to_lowercase()
            + maturation.timestamp().to_string().as_str();
        let event = &self.asset_pair_info.event_descriptor;
        let digits = event.nb_digits;
        let mut sk_nonces = Vec::with_capacity(digits.into());
        let mut nonces = Vec::with_capacity(digits.into());
        for _ in 0..digits {
            let mut sk_nonce = [0u8; 32];
            rng.fill_bytes(&mut sk_nonce);
            let oracle_r_kp = secp256k1_zkp::Keypair::from_seckey_slice(&SECP, &sk_nonce).unwrap();
            let nonce = XOnlyPublicKey::from_keypair(&oracle_r_kp).0;
            sk_nonces.push(sk_nonce);
            nonces.push(nonce);
        }
        let oracle_event = OracleEvent {
            oracle_nonces: nonces,
            event_maturity_epoch: maturation.timestamp() as u32,
            event_descriptor: EventDescriptor::DigitDecompositionEvent(
                self.asset_pair_info.event_descriptor.clone(),
            ),
            event_id,
        };

        let announcement = OracleAnnouncement {
            announcement_signature: SECP.sign_schnorr(
                &Message::from_digest(sha256::Hash::hash(&oracle_event.encode()).to_byte_array()),
                &self.keypair,
            ),
            oracle_public_key: self.keypair.public_key().into(),
            oracle_event,
        };
        Ok((announcement, sk_nonces))
    }

    /// Prepare announcements for a list of maturities and return them in the same order
    fn prepare_announcements(
        &self,
        maturations: &[DateTime<Utc>],
    ) -> Result<Vec<OracleAnnouncementWithSkNonces>> {
        let mut rng = thread_rng();
        maturations
            .iter()
            .map(|&maturity| self.prepare_announcement(maturity, &mut rng))
            .collect()
    }

    /// Create an oracle announcement that it will sign the price at given maturity instant
    pub async fn create_announcement(
        &self,
        maturation: DateTime<Utc>,
    ) -> Result<OracleAnnouncement> {
        // Check if the event was already announced
        let event_id = self.asset_pair_info.asset_pair.to_string().to_lowercase()
            + maturation.timestamp().to_string().as_str();
        if let Some(event) = self.db.get_event(&event_id).await? {
            info!(
                "Event {} already announced (should be possible only in debug mode or when restarted)",
                &event_id
            );
            return Ok(compute_announcement(self, event));
        }

        let (announcement, sk_nonces) = self.prepare_announcement(maturation, &mut thread_rng())?;

        self.db
            .insert_announcement(&announcement, sk_nonces)
            .await?;

        debug!("created oracle announcement with maturation {}", maturation);

        trace!("announcement {:#?}", &announcement);

        Ok(announcement)
    }

    pub async fn create_many_announcements(
        &self,
        maturations: &[DateTime<Utc>],
        chunk_size: usize,
    ) -> Result<()> {
        // Get the maturities that were not announced
        let non_existing_sorted_maturations = self
            .db
            .get_non_existing_sorted_maturity(maturations)
            .await?;

        // Check if all the events were already announced
        if non_existing_sorted_maturations.is_empty() {
            info!(
                "The {} maturations are already announced",
                maturations.len()
            );
            return Ok(());
        }

        // Create a stream that divides pending maturations into chunks
        // Each chunk is mapped to an async operation that processes the maturations with all oracles
        // The Ok wrapper is needed because try_buffer_unordered expects a Result type
        let chunks_stream = stream::iter(non_existing_sorted_maturations.chunks(chunk_size).map(
            |processing_mats| {
                let announcements_with_sk_nonces = self.prepare_announcements(processing_mats)?;
                Ok(async move {
                    // The announcements are already sorted because processing_mats is a chunk
                    // of the already sorted by postgres non_existing_sorted_maturations vector
                    // and prepare_announcements return announcements in the same order as maturities
                    self.db
                        .insert_many_announcements(&announcements_with_sk_nonces)
                        .await?;

                    debug!(
                        "created oracle announcements with maturation {:?}",
                        maturations
                    );
                    trace!(
                        "announcements {:#?}",
                        &announcements_with_sk_nonces
                            .into_iter()
                            .map(|(announcement, _)| announcement)
                            .collect::<Vec<_>>()
                    );
                    Ok(())
                })
            },
        ));

        // Process chunks concurrently with a maximum of 2 chunks being processed at once
        // This helps prevent database connection pool exhaustion while maintaining throughput
        // try_buffer_unordered does not guarantee ordering of results but allows more efficient concurrent processing
        let processing_stream = chunks_stream.try_buffered_unordered(2);

        // Collect all processed chunks and return any error
        processing_stream.try_collect().await
    }

    /// Attest or return attestation of event with given eventID. Return None if it was not announced, a PriceFeeder error if it the outcome is not available.
    /// Store in DB and return some oracle attestation if event is attested successfully.
    pub async fn try_attest_event(&self, event_id: &str) -> Result<Option<OracleAttestation>> {
        let Some(event) = self.db.get_event(event_id).await? else {
            return Ok(None);
        };
        let ScalarsRecords::DigitsSkNonce(outstanding_sk_nonces) = event.scalars_records else {
            info!(
                "Event {} already attested (should be possible only in debug mode)",
                event_id
            );
            return Ok(compute_attestation(self, event_id, event));
        };
        trace!("retrieving price feed for attestation");
        let outcome = self
            .asset_pair_info
            .pricefeed
            .get_pricefeed()
            .retrieve_price(self.asset_pair_info.asset_pair, event.maturity)
            .await?;

        let outcomes = to_digit_decomposition_vec(outcome, event.digits, event.precision);
        let (outcome_vec, signatures): (Vec<String>, Vec<Signature>) = outcomes
            .into_iter()
            .zip(outstanding_sk_nonces.iter())
            .map(|(outcome, outstanding_sk_nonce)| {
                let signature = sign_outcome(&SECP, &self.keypair, &outcome, outstanding_sk_nonce);
                (outcome, signature)
            })
            .unzip();

        let attestation = OracleAttestation {
            event_id: event_id.to_string(),
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes: outcome_vec,
        };
        debug!(
            "created oracle attestation with maturation {}",
            event.maturity
        );
        trace!("attestation {:#?}", &attestation);

        self.db
            .update_to_attestation(event_id, &attestation, outcome)
            .await?;

        Ok(Some(attestation))
    }

    /// If it exists, return an event announcement and attestation.
    pub async fn oracle_state(
        &self,
        event_id: &str,
    ) -> Result<Option<(OracleAnnouncement, Option<OracleAttestation>)>> {
        let Some(event) = self.db.get_event(event_id).await? else {
            return Ok(None);
        };

        let announcement = compute_announcement(self, event.clone());
        Ok(Some((
            announcement,
            compute_attestation(self, event_id, event),
        )))
    }

    /// If it exists, return many events announcement and attestation.
    pub async fn oracle_many_announcements(
        &self,
        events_ids: Vec<String>,
    ) -> Result<Box<[OracleAnnouncement]>> {
        let event = self
            .db
            .get_many_events(events_ids)
            .await?
            .ok_or(OracleError::MissingAnnouncements)?;

        Ok(event
            .into_iter()
            .map(|e| compute_announcement(self, e))
            .collect())
    }

    pub async fn force_new_attest_with_price(
        &self,
        maturation: DateTime<Utc>,
        price: f64,
    ) -> Result<(OracleAnnouncement, OracleAttestation)> {
        let event_id = (AssetPair::default().to_string() + &maturation.timestamp().to_string())
            .into_boxed_str();
        let event = &self.asset_pair_info.event_descriptor;
        let digits = event.nb_digits;
        let (sk_nonces, nonces, was_not_announced) = match self.db.get_event(&event_id).await? {
            Some(postgres_response) => match postgres_response.scalars_records {
                ScalarsRecords::DigitsSkNonce(sk_nonces) => {
                    info!(
                        "!!! Forced announcement !!!: {} event is already announced, will attest it immediately with price outcome {}",
                        event_id, price
                    );
                    let nonces = sk_nonces
                        .iter()
                        .map(|sk| {
                            let oracle_r_kp = secp256k1_zkp::Keypair::from_seckey_slice(&SECP, sk)
                                .expect("too low probability of secret to be invalid");
                            XOnlyPublicKey::from_keypair(&oracle_r_kp).0
                        })
                        .collect::<Vec<_>>();
                    (sk_nonces, nonces, false)
                }
                ScalarsRecords::DigitsAttestations(outcome, _) => {
                    info!(
                        "!!! Forced attestation !!!: {} event is already attested with price {}, ignore forcing",
                        event_id, outcome
                    );
                    let (oracle_announcement, oracle_attestation) =
                        self.oracle_state(&event_id).await?.expect("is announced");

                    return Ok((
                        oracle_announcement,
                        oracle_attestation.expect("is attested"),
                    ));
                }
            },
            None => {
                let mut sk_nonces = Vec::with_capacity(digits.into());
                let mut nonces = Vec::with_capacity(digits.into());
                debug!(
                    "!!! Forced announcement !!!: created oracle event and announcement with maturation {}",
                    maturation
                );
                // Begin scope to ensure ThreadRng is drop at compile time so that Oracle derive Send AutoTrait
                {
                    let mut rng = thread_rng();
                    for _ in 0..digits {
                        let mut sk_nonce = [0u8; 32];
                        rng.fill_bytes(&mut sk_nonce);
                        let oracle_r_kp =
                            secp256k1_zkp::Keypair::from_seckey_slice(&SECP, &sk_nonce).unwrap();
                        let nonce = XOnlyPublicKey::from_keypair(&oracle_r_kp).0;
                        sk_nonces.push(sk_nonce);
                        nonces.push(nonce);
                    }
                }; // End scope: ThreadRng is drop at compile time so that Oracle derives Send AutoTrait
                (sk_nonces, nonces, true)
            }
        };
        let oracle_event = OracleEvent {
            oracle_nonces: nonces,
            event_maturity_epoch: maturation.timestamp() as u32,
            event_descriptor: EventDescriptor::DigitDecompositionEvent(
                self.asset_pair_info.event_descriptor.clone(),
            ),
            event_id: self.asset_pair_info.asset_pair.to_string().to_lowercase()
                + maturation.timestamp().to_string().as_str(),
        };

        let announcement = OracleAnnouncement {
            announcement_signature: SECP.sign_schnorr(
                &Message::from_digest(sha256::Hash::hash(&oracle_event.encode()).to_byte_array()),
                &self.keypair,
            ),
            oracle_public_key: self.keypair.public_key().into(),
            oracle_event,
        };

        if was_not_announced {
            let _ = &self
                .db
                .insert_announcement(&announcement, sk_nonces.clone())
                .await?;
        }

        let outcomes = to_digit_decomposition_vec(
            price,
            digits,
            event
                .precision
                .try_into()
                .expect("Number range good in forced case"),
        );

        let (outcome_vec, signatures): (Vec<String>, Vec<Signature>) = outcomes
            .into_iter()
            .zip(sk_nonces.iter())
            .map(|(outcome, outstanding_sk_nonce)| {
                let signature = sign_outcome(&SECP, &self.keypair, &outcome, outstanding_sk_nonce);
                (outcome, signature)
            })
            .unzip();

        let attestation = OracleAttestation {
            event_id: announcement.oracle_event.event_id.clone(),
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes: outcome_vec,
        };

        info!(
            "!!! Forced attestation !!!: attested from announcement {:?} with price outcome {}, giving the following attestation: {:?}",
            &announcement, price, attestation
        );
        let _ = &self
            .db
            .update_to_attestation(event_id.as_ref(), &attestation, price)
            .await?;

        Ok((announcement, attestation))
    }
}

fn compute_attestation(
    oracle: &Oracle,
    event_id: &str,
    event: PostgresResponse,
) -> Option<OracleAttestation> {
    if let ScalarsRecords::DigitsAttestations(outcome, sigs) = event.scalars_records {
        let full_signatures = sigs
            .into_iter()
            .zip(event.nonce_public)
            .map(|(s, n)| {
                <(NoncePoint, SigningScalar) as Into<OracleSignature>>::into((n.into(), s.into()))
                    .into()
            })
            .collect();
        Some(OracleAttestation {
            event_id: event_id.to_string(),
            oracle_public_key: oracle.keypair.public_key().into(),
            signatures: full_signatures,
            outcomes: to_digit_decomposition_vec(outcome, event.digits, event.precision),
        })
    } else {
        None
    }
}

fn compute_announcement(oracle: &Oracle, event: PostgresResponse) -> OracleAnnouncement {
    let event_id = oracle.asset_pair_info.asset_pair.to_string().to_lowercase()
        + event.maturity.timestamp().to_string().as_str();

    let oracle_event = OracleEvent {
        oracle_nonces: event.nonce_public.clone(),
        event_maturity_epoch: event.maturity.timestamp() as u32,
        event_descriptor: EventDescriptor::DigitDecompositionEvent(
            oracle.asset_pair_info.event_descriptor.clone(),
        ),
        event_id,
    };

    OracleAnnouncement {
        announcement_signature: event.announcement_signature,
        oracle_public_key: oracle.keypair.public_key().into(),
        oracle_event,
    }
}
