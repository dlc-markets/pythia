use crate::{
    data_models::{
        asset_pair::AssetPair,
        event_ids::EventId,
        oracle_msgs::{Announcement, Attestation, DigitDecompositionEventDesc, Event},
    },
    oracle::crypto::{sign_event, sign_outcome},
    pricefeeds::ImplementedPriceFeed,
    AssetPairInfo,
};

use chrono::{DateTime, Utc};
use futures::{stream, StreamExt, TryStreamExt};
use futures_buffered::{BufferedStreamExt, FuturesUnorderedBounded};
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

    fn prepare_event_to_insert<'a>(
        &'a self,
        maturation: DateTime<Utc>,
        rng: &'a mut ThreadRng,
    ) -> impl Iterator<Item = SignedEventToInsert> + use<'a> {
        let event_ids = self
            .asset_pair_info
            .pricefeed
            .events_at_date(self.asset_pair_info.asset_pair, maturation)
            .into_iter();

        let event_ref = &self.asset_pair_info.event_descriptor;
        let digits = event_ref.nb_digits as usize;

        let mut secret_bytes = [0u8; 32];

        event_ids.map(move |event_id| {
            let mut nonces_keypairs = Vec::with_capacity(digits);

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

            SignedEventToInsert {
                announcement_signature: sign_event(&self.keypair, &oracle_event),
                nonces_keypairs,
                maturity: oracle_event.maturity,
                event_id,
                event_descriptor: oracle_event.event_descriptor,
            }
        })
    }

    /// Prepare announcements for a list of maturities and return them in the same order
    fn prepare_events_to_insert(&self, maturations: &[DateTime<Utc>]) -> Vec<SignedEventToInsert> {
        let mut rng = thread_rng();

        let average_attestation_per_maturity =
            if self.asset_pair_info.pricefeed == (ImplementedPriceFeed::Deribit) {
                12
            } else {
                1
            };

        let mut buffer_result =
            Vec::with_capacity(maturations.len() * average_attestation_per_maturity);
        // We must extend the buffer_result manually because prepare_announcement is an iterator
        // holding a mutable ref to rng which prevent us to flatten the iterators and collect.
        maturations.iter().copied().for_each(|maturity| {
            buffer_result.extend(self.prepare_event_to_insert(maturity, &mut rng));
        });

        buffer_result
    }

    /// Create oracle announcements that the oracle will sign the price at given maturity instant
    pub async fn create_announcements_at_date(
        &self,
        maturation: DateTime<Utc>,
    ) -> Result<Vec<Announcement>> {
        let mut rng = thread_rng();
        self.prepare_event_to_insert(maturation, &mut rng).map(async |event_to_insert| {
            // If already announced just return the announcement in DB
            Ok(match self.db.get_event(event_to_insert.event_id).await? {
                Some(event) => {
                    info!(
                        "Event {} already announced (should be possible only in debug mode or when restarted)",
                        &event_to_insert.event_id
                    );
                    compute_announcement(self, event_to_insert.event_id, event)
                },
                None => { self.db
                    .insert_announcement(&event_to_insert)
                    .await?;

                    debug!("created oracle announcement with maturation {maturation}");

                    trace!("inserted event {:#?}", &event_to_insert);

                    event_to_insert.as_announcement(self.get_public_key())
                }
            })
        })
        .collect::<FuturesUnorderedBounded<_>>()
        .try_collect()
        .await
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

        let average_announcement_per_maturity =
            if self.asset_pair_info.pricefeed == (ImplementedPriceFeed::Deribit) {
                13
            } else {
                1
            };

        // Create a stream that divides pending maturations into chunks
        // Each chunk is mapped to an async operation that processes the maturations with all oracles
        let chunks_stream = stream::iter(
            non_existing_sorted_maturations
                .chunks(CHUNK_SIZE / average_announcement_per_maturity + 1)
                .map(type_hint(async |processing_mats| {
                    let mut events_to_insert = self.prepare_events_to_insert(processing_mats);

                    events_to_insert.sort_by(|a, b| a.event_id.cmp(&b.event_id));
                    // We just sorted the announcements by event_id, so we can insert them in DB

                    self.db.insert_many_announcements(&events_to_insert).await?;

                    debug!("created oracle announcements with maturation {processing_mats:?}");
                    trace!("Inserted events: {events_to_insert:#?}");
                    Ok(())
                })),
        );

        // buffer_unordered with a maximum of 2 chunks being processed concurrently
        // helps prevent database connection pool exhaustion while maintaining throughput.
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

        let prices = self
            .asset_pair_info
            .pricefeed
            .retrieve_prices(self.asset_pair_info.asset_pair, event.maturity)
            .await?;

        let event_index = prices
            .binary_search_by(|(id, _)| id.cmp(&event_id))
            .map_err(|_| OracleError::MissingEventId(event_id))?;

        let outcome = prices[event_index]
            .1
            .ok_or(OracleError::MissingEventId(event_id))?;

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

    /// Attest or return attestations of all events at given date. Failing to attest an event will silently skip it.
    /// Store in DB and return its oracle attestation if an event is attested successfully.
    pub async fn attest_at_date(&self, date: DateTime<Utc>) -> Result<Vec<Result<Attestation>>> {
        trace!("retrieving prices feed for all attestations for date {date}");

        let prices = self
            .asset_pair_info
            .pricefeed
            .retrieve_prices(self.asset_pair_info.asset_pair, date)
            .await?;

        trace!("retrieved prices for events {prices:?}");

        let attestation_futures = prices.into_iter().map(async |(event_id, maybe_price)| {
            let price = maybe_price.ok_or(OracleError::MissingEventId(event_id))?;

            let event = self
                .db
                .get_event(event_id)
                .await
                .inspect_err(|e| {
                    error!("Postgres error while getting event {event_id}: {e:?}");
                })?
                .ok_or(OracleError::MissingAnnouncements)?;

            let ScalarsRecords::DigitsSkNonce(outstanding_sk_nonces) = event.scalars_records else {
                info!("Event {event_id} already attested (should be possible only in debug mode)");
                return Ok(compute_attestation(self, event_id, event).expect("is attested"));
            };

            let outcomes = to_digit_decomposition_vec(price, event.digits, event.precision);
            let signatures = outcomes
                .iter()
                .zip(outstanding_sk_nonces.into_iter())
                .map(|(outcome, outstanding_sk_nonce)| {
                    sign_outcome(&SECP, &self.keypair, *outcome, &outstanding_sk_nonce)
                })
                .collect::<Vec<_>>();

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
            trace!("attestation {:#?}", &attestation);

            self.db
                .update_to_attestation(attestation.event_id, &attestation, price)
                .await
                .inspect_err(|e| {
                    error!("Postgres error while updating to attestation {event_id}: {e:?}");
                })?;

            Ok(attestation)
        });

        Ok(attestation_futures
            .collect::<FuturesUnorderedBounded<_>>()
            .collect()
            .await)
    }

    /// If it exists, return an event announcement and attestation.
    pub async fn oracle_state(
        &self,
        event_id: EventId,
    ) -> Result<Option<(Announcement, Option<Attestation>)>> {
        let Some(event) = self.db.get_event(event_id).await? else {
            return Ok(None);
        };

        let announcement = compute_announcement(self, event_id, event.clone());
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
            .map(|(e, event_id)| compute_announcement(self, event_id, e))
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
                        Some(compute_announcement(self, event_id, postgres_response)),
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

fn compute_announcement(
    oracle: &Oracle,
    event_id: EventId,
    event: PostgresResponse,
) -> Announcement {
    let oracle_event = Event {
        oracle_nonces: event.nonces_public.clone(),
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
