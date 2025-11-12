use crate::{
    AssetPairInfo, DBconnection,
    data_models::{
        event_ids::EventId,
        oracle_msgs::{Announcement, Attestation, Event},
    },
    db::{EventFromPostgres, ScalarsRecords},
    oracle::crypto::{join_sig, sign_event, sign_outcome, to_digit_decomposition_vec},
};

use secp256k1_zkp::{Keypair, XOnlyPublicKey};

pub mod announcing;
pub mod attesting;
pub mod state;

mod crypto;
pub(crate) mod error;

mod force;
#[cfg(test)]
mod test;

use error::{OracleError, Result};
/// Number of maturations to process in each batch to prevent database connection pool exhaustion
/// and maintain optimal performance while processing backlogged announcements
pub const CHUNK_SIZE: usize = 100;

/// A stateful digits event oracle application. It prepares announcements and try to attest them on demand. It also managed the storage of announcements and attestations.
#[derive(Clone)]
pub struct Oracle {
    /// Oracle attestation event format summary
    pub asset_pair_info: AssetPairInfo,
    keypair: Keypair,
}

impl Oracle {
    /// Create a new instance of oracle for a numerical outcome given a postgres DB connection and a keypair.
    pub fn new(asset_pair_info: AssetPairInfo, keypair: Keypair) -> Oracle {
        Oracle {
            asset_pair_info,
            keypair,
        }
    }
    /// The oracle public key
    pub fn get_public_key(&self) -> XOnlyPublicKey {
        self.keypair.x_only_public_key().0
    }
    /// Check if the oracle announced at least one event
    pub async fn is_empty(&self, db: &DBconnection) -> Result<bool> {
        db.is_empty().await.map_err(OracleError::from)
    }

    fn compute_announcement(&self, event: EventFromPostgres) -> Announcement {
        let event_id = event.event_id_infos.as_event_id();

        let oracle_event = Event {
            oracle_nonces: event.nonces_public.into_vec(),
            maturity: event.maturity.timestamp() as u32,
            event_descriptor: self.asset_pair_info.event_descriptor,
            event_id,
        };

        Announcement {
            announcement_signature: event.announcement_signature,
            oracle_public_key: self.keypair.x_only_public_key().0,
            oracle_event,
        }
    }

    fn compute_attestation(&self, event: &EventFromPostgres) -> Option<Attestation> {
        let ScalarsRecords::DigitsAttestations(outcome, sigs) = &event.scalars_records else {
            return None;
        };

        let signatures = sigs
            .iter()
            .zip(&event.nonces_public)
            .map(|(&scalar, &nonce)| join_sig(nonce, scalar))
            .collect();

        Some(Attestation {
            event_id: event.event_id_infos.as_event_id(),
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes: to_digit_decomposition_vec(*outcome, event.digits, event.precision),
        })
    }

    /// If it exists, return an event announcement and attestation.
    pub async fn oracle_state(
        &self,
        db: &DBconnection,
        event_id: EventId,
    ) -> Result<Option<(Announcement, Option<Attestation>)>> {
        Ok(db.get_event(event_id).await?.map(|event| {
            let attestation = self.compute_attestation(&event);
            (self.compute_announcement(event), attestation)
        }))
    }

    /// If it exists, return many events announcement and attestation.
    pub async fn oracle_many_announcements(
        &self,
        db: &DBconnection,
        events_ids: &[EventId],
    ) -> Result<Box<[Announcement]>> {
        let events_iter = db.get_events_with(events_ids).await?;

        if events_iter.len() != events_ids.len() {
            Err(OracleError::MissingAnnouncements)
        } else {
            Ok(events_iter.map(|e| self.compute_announcement(e)).collect())
        }
    }
}

impl DBconnection {
    async fn get_event(&self, event_id: EventId) -> Result<Option<EventFromPostgres>> {
        Ok(self
            .get_events_with(core::slice::from_ref(&event_id))
            .await?
            .next())
    }
}
