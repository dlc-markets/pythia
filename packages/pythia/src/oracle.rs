use crate::{
    data_models::{
        asset_pair::AssetPair,
        event_ids::EventId,
        oracle_msgs::{Announcement, Attestation, DigitDecompositionEventDesc, Event},
    },
    oracle::crypto::{sign_event, sign_outcome},
    AssetPairInfo,
};

use chrono::{DateTime, Utc};
use futures::{stream, TryStreamExt};
use futures_buffered::BufferedStreamExt;
use secp256k1_zkp::{
    rand::{rngs::ThreadRng, thread_rng, RngCore},
    schnorr::Signature,
    Keypair, XOnlyPublicKey,
};

use crate::SECP;

mod crypto;
pub(crate) mod error;
pub(crate) mod postgres;
#[cfg(test)]
mod test;

use crypto::{to_digit_decomposition_vec, OracleSignature};
use error::{OracleError, Result};
use postgres::*;
/// Number of maturations to process in each batch to prevent database connection pool exhaustion
/// and maintain optimal performance while processing backlogged announcements
pub const CHUNK_SIZE: usize = 100;

#[derive(Debug)]
struct SignedEventToInsert {
    announcement_signature: Signature,
    nonces_keypairs: Vec<Keypair>,
    maturity: u32,
    event_id: EventId,
    event_descriptor: DigitDecompositionEventDesc,
}

impl SignedEventToInsert {
    fn as_announcement(&self, oracle_public_key: XOnlyPublicKey) -> Announcement {
        Announcement {
            announcement_signature: self.announcement_signature,
            oracle_public_key,
            oracle_event: Event {
                oracle_nonces: self
                    .nonces_keypairs
                    .iter()
                    .map(|kp| kp.x_only_public_key().0.serialize())
                    .collect(),
                maturity: self.maturity,
                event_id: self.event_id,
                event_descriptor: self.event_descriptor,
            },
        }
    }
}

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
        self.keypair.x_only_public_key().0
    }
    /// Check if the oracle announced at least one event
    pub async fn is_empty(&self) -> Result<bool> {
        self.db.is_empty().await.map_err(OracleError::from)
    }

    fn prepare_event_to_insert(
        &self,
        maturation: DateTime<Utc>,
        rng: &mut ThreadRng,
    ) -> Result<SignedEventToInsert> {
        let event_id =
            EventId::spot_from_pair_and_timestamp(self.asset_pair_info.asset_pair, maturation);

        let event = &self.asset_pair_info.event_descriptor;
        let digits = event.nb_digits;
        let mut nonces_keypairs = Vec::with_capacity(digits.into());
        let mut secret_bytes = [0u8; 32];
        for _ in 0..digits {
            rng.fill_bytes(&mut secret_bytes);
            let oracle_r_kp = secp256k1_zkp::Keypair::from_seckey_slice(&SECP, &secret_bytes)
                .expect("secret_bytes has the required length");
            nonces_keypairs.push(oracle_r_kp);
        }

        let oracle_event = Event {
            oracle_nonces: nonces_keypairs
                .iter()
                .map(|kp| kp.x_only_public_key().0.serialize())
                .collect(),
            maturity: maturation.timestamp() as u32,
            event_id,
            event_descriptor: self.asset_pair_info.event_descriptor,
        };

        Ok(SignedEventToInsert {
            announcement_signature: sign_event(&self.keypair, &oracle_event),
            nonces_keypairs,
            maturity: oracle_event.maturity,
            event_id: oracle_event.event_id,
            event_descriptor: oracle_event.event_descriptor,
        })
    }

    /// Prepare announcements for a list of maturities and return them in the same order
    fn prepare_events_to_insert(
        &self,
        maturations: &[DateTime<Utc>],
    ) -> Result<Vec<SignedEventToInsert>> {
        let mut rng = thread_rng();

        let events_to_insert = maturations
            .iter()
            .map(|&maturity| self.prepare_event_to_insert(maturity, &mut rng))
            .collect::<Result<_>>()?;

        Ok(events_to_insert)
    }

    /// Create an oracle announcement that it will sign the price at given maturity instant
    pub async fn create_announcement(&self, maturation: DateTime<Utc>) -> Result<Announcement> {
        // Check if the event was already announced
        let event_id =
            EventId::spot_from_pair_and_timestamp(self.asset_pair_info.asset_pair, maturation);
        if let Some(event) = self.db.get_event(event_id).await? {
            info!(
                "Event {} already announced (should be possible only in debug mode or when restarted)",
                &event_id
            );
            return Ok(compute_announcement(self, event));
        }

        let event_to_insert = self.prepare_event_to_insert(maturation, &mut thread_rng())?;

        self.db.insert_announcement(&event_to_insert).await?;

        let announcement = event_to_insert.as_announcement(self.get_public_key());

        debug!("created oracle announcement with maturation {maturation}");

        trace!("announcement {announcement:#?}");

        Ok(announcement)
    }

    pub async fn create_many_announcements<const CHUNK_SIZE: usize>(
        &self,
        maturations: &[DateTime<Utc>],
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
        let chunks_stream = stream::iter(non_existing_sorted_maturations.chunks(CHUNK_SIZE).map(
            type_hint(async |processing_mats| {
                let events_to_insert = self.prepare_events_to_insert(processing_mats)?;
                // The announcements are already sorted because processing_mats is a chunk
                // of the already sorted by postgres non_existing_sorted_maturations vector
                // and prepare_announcements return announcements in the same order as maturities
                self.db.insert_many_announcements(&events_to_insert).await?;

                debug!("created oracle announcements with maturation {processing_mats:?}");
                trace!("announcements {events_to_insert:#?}");
                Ok(())
            }),
        ));

        // buffer_unordered allows a maximum of 2 chunks being processed concurrently.
        // This helps prevent database connection pool exhaustion while maintaining throughput.
        // Process all chunks in any order and return any error.
        chunks_stream.buffered_unordered(2).try_collect().await
    }

    /// Attest or return attestation of event with given eventID. Return None if it was not announced, a PriceFeeder error if it the outcome is not available.
    /// Store in DB and return some oracle attestation if event is attested successfully.
    pub async fn try_attest_event(&self, event_id: EventId) -> Result<Option<Attestation>> {
        let Some(event) = self.db.get_event(event_id).await? else {
            return Ok(None);
        };
        let ScalarsRecords::DigitsSkNonce(outstanding_sk_nonces) = event.scalars_records else {
            info!("Event {event_id} already attested (should be possible only in debug mode)");
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
        let signatures = outcomes
            .iter()
            .copied()
            .zip(outstanding_sk_nonces.iter())
            .map(|(outcome, outstanding_sk_nonce)| {
                sign_outcome(&SECP, &self.keypair, outcome, outstanding_sk_nonce)
            })
            .collect::<Vec<Signature>>();

        let attestation = Attestation {
            event_id,
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes,
        };
        debug!(
            "created oracle attestation with maturation {}",
            event.maturity
        );
        trace!("attestation {attestation:#?}");

        self.db
            .update_to_attestation(event_id, &attestation, outcome)
            .await?;

        Ok(Some(attestation))
    }

    /// If it exists, return an event announcement and attestation.
    pub async fn oracle_state(
        &self,
        event_id: EventId,
    ) -> Result<Option<(Announcement, Option<Attestation>)>> {
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
        events_ids: Vec<EventId>,
    ) -> Result<Box<[Announcement]>> {
        let event = self
            .db
            .get_many_events(events_ids)
            .await?
            .ok_or(OracleError::MissingAnnouncements)?;

        Ok(event
            .into_iter()
            .map(|e| compute_announcement(self, e.0))
            .collect())
    }

    pub async fn force_new_attest_with_price(
        &self,
        maturation: DateTime<Utc>,
        price: f64,
    ) -> Result<(Announcement, Attestation)> {
        let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::default(), maturation);
        let event = &self.asset_pair_info.event_descriptor;
        let digits = event.nb_digits;
        let (nonces_keypairs, maybe_announcement) = match self.db.get_event(event_id).await? {
            Some(postgres_response) => match &postgres_response.scalars_records {
                ScalarsRecords::DigitsSkNonce(sk_nonces) => {
                    info!(
                        "!!! Forced announcement !!!: {event_id} event is already announced, will attest it immediately with price outcome {price}"
                    );
                    let as_keypairs = sk_nonces
                        .iter()
                        .map(|sk| {
                            secp256k1_zkp::Keypair::from_seckey_slice(&SECP, sk)
                                .expect("too low probability of secret to be invalid")
                        })
                        .collect::<Vec<_>>();
                    (
                        as_keypairs,
                        Some(compute_announcement(self, postgres_response)),
                    )
                }
                ScalarsRecords::DigitsAttestations(outcome, _) => {
                    info!(
                        "!!! Forced attestation !!!: {event_id} event is already attested with price {outcome}, ignore forcing"
                    );
                    let (oracle_announcement, oracle_attestation) =
                        self.oracle_state(event_id).await?.expect("is announced");

                    return Ok((
                        oracle_announcement,
                        oracle_attestation.expect("is attested"),
                    ));
                }
            },
            None => {
                let mut nonces_keypairs = Vec::with_capacity(digits.into());
                debug!(
                    "!!! Forced announcement !!!: created oracle event and announcement with maturation {maturation}"
                );
                // Begin scope to ensure ThreadRng is drop at compile time so that Oracle derive Send AutoTrait
                {
                    let mut rng = thread_rng();
                    let mut secret_bytes = [0u8; 32];
                    for _ in 0..digits {
                        rng.fill_bytes(&mut secret_bytes);
                        let oracle_r_kp =
                            secp256k1_zkp::Keypair::from_seckey_slice(&SECP, &secret_bytes)
                                .expect("secret_bytes has the required length");
                        nonces_keypairs.push(oracle_r_kp);
                    }
                }; // End scope: ThreadRng is drop at compile time so that Oracle derives Send AutoTrait
                (nonces_keypairs, None)
            }
        };

        let outcomes = to_digit_decomposition_vec(
            price,
            digits,
            event
                .precision
                .try_into()
                .expect("Number range good in forced case"),
        );

        let signatures = outcomes
            .iter()
            .zip(nonces_keypairs.iter())
            .map(|(outcome, nonce_keypair)| {
                sign_outcome(
                    &SECP,
                    &self.keypair,
                    *outcome,
                    &nonce_keypair.secret_bytes(),
                )
            })
            .collect::<Vec<Signature>>();

        let announcement = if let Some(announcement) = maybe_announcement {
            announcement
        } else {
            let oracle_event = Event {
                oracle_nonces: nonces_keypairs
                    .iter()
                    .map(|kp| kp.x_only_public_key().0.serialize())
                    .collect(),
                maturity: maturation.timestamp() as u32,
                event_descriptor: self.asset_pair_info.event_descriptor,
                event_id,
            };

            let event_to_insert = SignedEventToInsert {
                announcement_signature: sign_event(&self.keypair, &oracle_event),
                nonces_keypairs,
                maturity: oracle_event.maturity,
                event_id,
                event_descriptor: oracle_event.event_descriptor,
            };
            let _ = &self.db.insert_announcement(&event_to_insert).await?;

            event_to_insert.as_announcement(self.get_public_key())
        };

        let attestation = Attestation {
            event_id,
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes,
        };

        info!(
            "!!! Forced attestation !!!: attested from announcement {:?} with price outcome {}, giving the following attestation: {:?}",
            &announcement, price, attestation
        );
        let _ = &self
            .db
            .update_to_attestation(event_id, &attestation, price)
            .await?;

        Ok((announcement, attestation))
    }
}

fn compute_attestation(
    oracle: &Oracle,
    event_id: EventId,
    event: PostgresResponse,
) -> Option<Attestation> {
    let ScalarsRecords::DigitsAttestations(outcome, sigs) = event.scalars_records else {
        return None;
    };

    let signatures = sigs
        .into_iter()
        .zip(event.nonces_public)
        .map(|parts| {
            OracleSignature::from(parts)
                .try_into()
                .expect("We only insert serialized XOnlyPublicKey in DB, if this happen DB has been corrupted in some very bad way")
        })
        .collect();

    Some(Attestation {
        event_id,
        oracle_public_key: oracle.keypair.public_key().into(),
        signatures,
        outcomes: to_digit_decomposition_vec(outcome, event.digits, event.precision),
    })
}

fn compute_announcement(oracle: &Oracle, event: PostgresResponse) -> Announcement {
    let event_id = EventId::spot_from_pair_and_timestamp(
        oracle.asset_pair_info.asset_pair,
        event.maturity.with_timezone(&Utc),
    );

    let oracle_event = Event {
        oracle_nonces: event.nonces_public,
        maturity: event.maturity.timestamp() as u32,
        event_descriptor: oracle.asset_pair_info.event_descriptor,
        event_id,
    };

    Announcement {
        announcement_signature: event.announcement_signature,
        oracle_public_key: oracle.keypair.x_only_public_key().0,
        oracle_event,
    }
}

/// Enforce the passed closure is generic over its lifetime
/// and Send for all lifetimes.
fn type_hint<T: ?Sized, F>(f: F) -> F
where
    F: for<'a> AsyncFn(&'a T) -> Result<()> + Send,
{
    f
}
