use chrono::{DateTime, Utc};
use futures::{TryStreamExt as _, stream};
use futures_buffered::{BufferedStreamExt as _, FuturesUnorderedBounded};
use secp256k1_zkp::{
    Keypair, XOnlyPublicKey,
    rand::{RngCore as _, rngs::ThreadRng, thread_rng},
    schnorr::Signature,
};

use crate::{
    DBconnection, SECP,
    data_models::{
        event_ids::EventIdInfos,
        oracle_msgs::{Announcement, DigitDecompositionEventDesc, Event},
    },
    oracle::{Oracle, error::Result, sign_event},
};

mod insert_announcement;

#[derive(Debug)]
pub(crate) struct SignedEventToInsert {
    pub announcement_signature: Signature,
    pub nonces_keypairs: Vec<Keypair>,
    pub maturity: u32,
    pub event_id_infos: EventIdInfos,
    pub event_descriptor: DigitDecompositionEventDesc,
}

impl SignedEventToInsert {
    pub fn as_announcement(&self, oracle_public_key: XOnlyPublicKey) -> Announcement {
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
                event_id: self.event_id_infos.as_event_id(),
                event_descriptor: self.event_descriptor,
            },
        }
    }
}

impl Oracle {
    /// Create oracle announcements that the oracle will sign the price at given maturity instant
    pub(crate) async fn create_announcements_at_date(
        &self,
        db: &DBconnection,
        maturation: DateTime<Utc>,
    ) -> Result<Vec<Announcement>> {
        let mut rng = thread_rng();
        self.prepare_event_to_insert(maturation, &mut rng).map(async |event_to_insert| {
            // If already announced just return the announcement in DB
            Ok(match db.get_event(event_to_insert.event_id_infos.as_event_id()).await? {
                Some(event) => {
                    info!(
                        "Event {} already announced (should be possible only in debug mode or when restarted)",
                        &event_to_insert.event_id_infos.as_event_id()
                    );
                    self.compute_announcement(event)
                },
                None => { db
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

    pub(crate) async fn create_many_announcements<const CHUNK_SIZE: usize>(
        &self,
        db: &DBconnection,
        maturations: &[DateTime<Utc>],
    ) -> Result<()> {
        // Get the maturities that were not announced
        let non_existing_sorted_maturations =
            db.get_non_existing_sorted_maturity(maturations).await?;

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
        let chunks_stream = stream::iter(
            non_existing_sorted_maturations
                .chunks(
                    CHUNK_SIZE.div_ceil(
                        self.asset_pair_info
                            .pricefeed
                            .average_events_per_maturity()
                            .get(),
                    ),
                )
                .map(type_hint(async |processing_mats| {
                    let mut events_to_insert = self.prepare_events_to_insert(processing_mats);

                    events_to_insert.sort_by(|a, b| {
                        a.event_id_infos
                            .as_event_id()
                            .cmp(&b.event_id_infos.as_event_id())
                    });
                    // We just sorted the announcements by event_id, so we can insert them in DB

                    db.insert_many_announcements(&events_to_insert).await?;

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

    fn prepare_event_to_insert<'a>(
        &'a self,
        maturation: DateTime<Utc>,
        rng: &'a mut ThreadRng,
    ) -> impl Iterator<Item = SignedEventToInsert> + use<'a> {
        let event_ids_infos = self
            .asset_pair_info
            .pricefeed
            .to_schedule_events(self.asset_pair_info.asset_pair, maturation)
            .into_iter();

        let event_ref = &self.asset_pair_info.event_descriptor;
        let digits = event_ref.nb_digits as usize;

        let mut secret_bytes = [0u8; 32];

        event_ids_infos.map(move |event_id_infos| {
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
                event_id: event_id_infos.as_event_id(),
                event_descriptor: self.asset_pair_info.event_descriptor,
            };

            SignedEventToInsert {
                announcement_signature: sign_event(&self.keypair, &oracle_event),
                nonces_keypairs,
                maturity: oracle_event.maturity,
                event_id_infos,
                event_descriptor: oracle_event.event_descriptor,
            }
        })
    }

    /// Prepare announcements for a list of maturities and return them in the same order
    fn prepare_events_to_insert(&self, maturations: &[DateTime<Utc>]) -> Vec<SignedEventToInsert> {
        let mut rng = thread_rng();

        let mut buffer_result = Vec::with_capacity(
            maturations.len()
                * self
                    .asset_pair_info
                    .pricefeed
                    .average_events_per_maturity()
                    .get(),
        );

        // We must extend the buffer_result manually because prepare_announcement is an iterator
        // holding a mutable ref to rng which prevent us to flatten the iterators and collect.
        maturations.iter().copied().for_each(|maturity| {
            buffer_result.extend(self.prepare_event_to_insert(maturity, &mut rng));
        });

        buffer_result
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

#[cfg(test)]
mod test;
