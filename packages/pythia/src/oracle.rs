use crate::{config::AssetPair, oracle::crypto::sign_outcome, AssetPairInfo};

use chrono::{DateTime, Utc};
use dlc_messages::oracle_msgs::{
    EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent,
};
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

use crypto::{to_digit_decomposition_vec, NoncePoint, OracleSignature, SigningScalar};
use error::{OracleError, Result};
use postgres::*;

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

    pub fn prepare_announcements(
        &self,
        maturations: &[DateTime<Utc>],
    ) -> Result<Vec<OracleAnnouncementWithSkNonces>> {
        let mut announcements: Vec<(OracleAnnouncement, Vec<[u8; 32]>)> = Vec::new();
        let mut rng = thread_rng();
        for maturation in maturations {
            let (announcement, sk_nonces) = self.prepare_announcement(*maturation, &mut rng)?;
            announcements.push((announcement, sk_nonces));
        }
        Ok(announcements)
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

    pub async fn create_many_announcements(&self, maturations: &[DateTime<Utc>]) -> Result<()> {
        // Check if all the events were already announced
        let non_existing_maturations = self.db.get_non_existing_maturity(maturations).await?;
        if non_existing_maturations.is_empty() {
            info!("All events already announced");
            return Ok(());
        }

        // Get the maturities that were not announced
        let announcements_with_sk_nonces = self.prepare_announcements(&non_existing_maturations)?;
        self.db
            .insert_many_announcements(&announcements_with_sk_nonces)
            .await?;
        let announcements = announcements_with_sk_nonces
            .into_iter()
            .map(|(announcement, _)| announcement)
            .collect::<Vec<_>>();
        debug!(
            "created oracle announcements with maturation {:?}",
            maturations
        );
        trace!("announcements {:#?}", &announcements);

        Ok(())
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

#[cfg(test)]
mod test {
    use chrono::{Duration, Utc};
    use dlc_messages::oracle_msgs::DigitDecompositionEventDescriptor;
    use secp256k1_zkp::{Keypair, SecretKey};
    use sqlx::PgPool;
    use std::str::FromStr;

    use crate::{
        oracle::{Oracle, ScalarsRecords},
        pricefeeds::ImplementedPriceFeed,
        AssetPair, AssetPairInfo, DBconnection, SECP,
    };
    type Error = Box<dyn std::error::Error>;
    type Result<T> = std::result::Result<T, Error>;

    fn create_test_oracle(db: &DBconnection) -> Result<Oracle> {
        // Create a test oracle
        let asset_pair_info = AssetPairInfo {
            pricefeed: ImplementedPriceFeed::Lnmarkets,
            asset_pair: AssetPair::default(),
            event_descriptor: DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: false,
                unit: "usd".to_owned(),
                precision: 0,
                nb_digits: 20,
            },
        };
        let secret_key = SecretKey::from_str(
            "d0a26c65de0b4b853432c3931ee280f67b9c52de33e1b3aecb04edc1ec40ef4a",
        )?;
        let keypair = Keypair::from_secret_key(&SECP, &secret_key);
        let oracle = Oracle::new(asset_pair_info.clone(), db.clone(), keypair);
        Ok(oracle)
    }

    fn create_test_oracle_with_digits(db: &DBconnection, nb_digit: u16) -> Result<Oracle> {
        // Create a test oracle
        let asset_pair_info = AssetPairInfo {
            pricefeed: ImplementedPriceFeed::Lnmarkets,
            asset_pair: AssetPair::default(),
            event_descriptor: DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: false,
                unit: "usd".to_owned(),
                precision: 0,
                nb_digits: nb_digit,
            },
        };
        let secret_key = SecretKey::from_str(
            "d0a26c65de0b4b853432c3931ee280f67b9c52de33e1b3aecb04edc1ec40ef4a",
        )?;
        let keypair = Keypair::from_secret_key(&SECP, &secret_key);
        let oracle = Oracle::new(asset_pair_info.clone(), db.clone(), keypair);
        Ok(oracle)
    }

    mod test_create_many_announcements {
        use super::*;

        #[sqlx::test]
        async fn test_create_many_announcements_new(pool: PgPool) -> Result<()> {
            let db = DBconnection(pool);

            // Create a test oracle
            let oracle = create_test_oracle(&db)?;

            let now = Utc::now();
            let maturations = [
                now + Duration::hours(1),
                now + Duration::hours(2),
                now + Duration::hours(3),
            ];

            oracle.create_many_announcements(&maturations).await?;

            // Verify that all announcements were created
            for maturation in maturations.iter() {
                let event_id = oracle.asset_pair_info.asset_pair.to_string().to_lowercase()
                    + maturation.timestamp().to_string().as_str();
                let event = db.get_event(&event_id).await?;
                assert!(
                    event.is_some(),
                    "event: {} should exist for maturation: {}",
                    event_id,
                    maturation
                );

                let event = event.expect("Event should exist");
                assert_eq!(
                    event.digits, oracle.asset_pair_info.event_descriptor.nb_digits,
                    "Event should have the correct number of digits"
                );
                assert_eq!(
                    event.precision as i32, oracle.asset_pair_info.event_descriptor.precision,
                    "Event should have the correct precision"
                );
                assert_eq!(
                    event.maturity.timestamp(),
                    maturation.timestamp(),
                    "Event should have the correct maturity"
                );

                // Verify it has nonces
                assert_eq!(
                    event.nonce_public.len() as u16,
                    oracle.asset_pair_info.event_descriptor.nb_digits,
                    "Event should have the correct number of nonces"
                );

                // Verify it's not attested yet
                match event.scalars_records {
                    ScalarsRecords::DigitsSkNonce(_) => {}
                    ScalarsRecords::DigitsAttestations(_, _) => {
                        panic!("Event should not be attested")
                    }
                }
            }
            Ok(())
        }

        #[sqlx::test]
        async fn test_create_many_announcements_already_exist(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);

            // Create maturations
            let oracle = create_test_oracle(&db)?;
            let now = Utc::now();
            let maturations = [now + Duration::hours(1), now + Duration::hours(2)];

            oracle.create_many_announcements(&maturations).await?;

            // Record the event IDs and nonces
            let mut event_ids = Vec::new();
            let mut original_nonces = Vec::new();

            for maturation in &maturations {
                let event_id = oracle.asset_pair_info.asset_pair.to_string().to_lowercase()
                    + maturation.timestamp().to_string().as_str();
                event_ids.push(event_id.clone());

                let event = db.get_event(&event_id).await?.unwrap();
                original_nonces.push(event.nonce_public.clone());
            }

            // Try creating announcements again
            oracle.create_many_announcements(&maturations).await?;

            // Verify nothing changed
            for (i, event_id) in event_ids.iter().enumerate() {
                let event = db.get_event(event_id).await?.unwrap();
                assert_eq!(
                    event.nonce_public, original_nonces[i],
                    "Nonces should not change when announcement already exists"
                );
            }

            Ok(())
        }

        #[sqlx::test]
        async fn test_create_many_announcements_mixed(pool: PgPool) -> Result<()> {
            // Create a DB connection and oracle
            let db = DBconnection(pool);
            let oracle = create_test_oracle(&db)?;

            // Create some initial announcements
            let now = Utc::now();
            let existing_maturations = [now + Duration::hours(1), now + Duration::hours(3)];
            oracle
                .create_many_announcements(&existing_maturations)
                .await?;

            // Now try with a mix of existing and new maturations
            let mixed_maturations = [
                now + Duration::hours(1), // Already exists
                now + Duration::hours(2), // New
                now + Duration::hours(3), // Already exists
                now + Duration::hours(4), // New
            ];

            // Create announcements
            oracle.create_many_announcements(&mixed_maturations).await?;

            // Verify all announcements exist
            for maturation in &mixed_maturations {
                let event_id = oracle.asset_pair_info.asset_pair.to_string().to_lowercase()
                    + maturation.timestamp().to_string().as_str();
                let event = db.get_event(&event_id).await?;
                assert!(
                    event.is_some(),
                    "Event should exist for maturation {}",
                    maturation
                );
                let events = db
                    .get_many_events([event_id].to_vec())
                    .await?
                    .unwrap_or_default();
                assert_eq!(events.len(), 1, "Event should be unique");
            }

            Ok(())
        }

        #[sqlx::test]
        async fn test_create_many_announcements_empty(pool: PgPool) -> Result<()> {
            // Create a DB connection and oracle
            let db = DBconnection(pool);
            let oracle = create_test_oracle(&db)?;

            // Try with empty array
            let result = oracle.create_many_announcements(&[]).await;

            // Should succeed with no issues
            assert!(result.is_ok());

            Ok(())
        }

        #[sqlx::test]
        async fn test_create_many_announcements_different_digit_counts(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);

            // Create two oracles with different digit counts
            let oracle20 = create_test_oracle_with_digits(&db, 20)?;
            let oracle10 = create_test_oracle_with_digits(&db, 10)?;

            // Create maturations
            // Note: There can't be 2 announcements with the same maturation time
            let now = Utc::now();
            let maturations_even = [now, now + Duration::hours(2), now + Duration::hours(4)];
            let maturations_odd = [now + Duration::hours(1), now + Duration::hours(3)];

            // Create announcements with both oracles
            oracle20
                .create_many_announcements(&maturations_even)
                .await?;
            oracle10.create_many_announcements(&maturations_odd).await?;

            // Verify announcements have correct digit counts
            for maturation in &maturations_even {
                // Check first oracle's announcement
                let event_id1 = oracle20
                    .asset_pair_info
                    .asset_pair
                    .to_string()
                    .to_lowercase()
                    + maturation.timestamp().to_string().as_str();
                let event1 = db.get_event(&event_id1).await?.unwrap();
                assert_eq!(event1.digits, 20);
                assert_eq!(event1.nonce_public.len(), 20);
            }

            for maturation in &maturations_odd {
                // Check second oracle's announcement
                let event_id2 = oracle10
                    .asset_pair_info
                    .asset_pair
                    .to_string()
                    .to_lowercase()
                    + maturation.timestamp().to_string().as_str();
                let event2 = db.get_event(&event_id2).await?.unwrap();
                assert_eq!(event2.digits, 10);
                assert_eq!(event2.nonce_public.len(), 10);
            }

            Ok(())
        }
    }

    mod test_prepare_announcement {
        use super::*;
        use bitcoin::hashes::{sha256, Hash};
        use dlc_messages::oracle_msgs::EventDescriptor;
        use lightning::util::ser::Writeable;
        use secp256k1_zkp::{rand::thread_rng, Message, XOnlyPublicKey};

        #[sqlx::test]
        fn test_prepare_announcement_basic(pool: PgPool) -> Result<()> {
            // Create a test oracle
            let db = DBconnection(pool);
            let oracle = create_test_oracle(&db)?;

            // Create a maturation time
            let maturation = Utc::now() + Duration::hours(1);

            // Prepare announcement
            let (announcement, sk_nonces) =
                oracle.prepare_announcement(maturation, &mut thread_rng())?;

            // Verify event ID format (asset_pair + timestamp)
            let expected_id = oracle.asset_pair_info.asset_pair.to_string().to_lowercase()
                + maturation.timestamp().to_string().as_str();
            assert_eq!(announcement.oracle_event.event_id, expected_id);

            // Verify maturation time
            assert_eq!(
                announcement.oracle_event.event_maturity_epoch as i64,
                maturation.timestamp()
            );

            // Verify event descriptor
            match &announcement.oracle_event.event_descriptor {
                EventDescriptor::DigitDecompositionEvent(desc) => {
                    assert_eq!(
                        desc.nb_digits,
                        oracle.asset_pair_info.event_descriptor.nb_digits
                    );
                    assert_eq!(
                        desc.precision,
                        oracle.asset_pair_info.event_descriptor.precision
                    );
                    assert_eq!(desc.base, oracle.asset_pair_info.event_descriptor.base);
                    assert_eq!(
                        desc.is_signed,
                        oracle.asset_pair_info.event_descriptor.is_signed
                    );
                }
                _ => panic!("Expected DigitDecompositionEvent"),
            }

            // Verify oracle public key
            assert_eq!(announcement.oracle_public_key, oracle.get_public_key());

            // Verify nonce count
            assert_eq!(
                announcement.oracle_event.oracle_nonces.len(),
                oracle.asset_pair_info.event_descriptor.nb_digits as usize
            );

            // Verify secret nonces length
            assert_eq!(
                sk_nonces.len(),
                oracle.asset_pair_info.event_descriptor.nb_digits as usize
            );

            Ok(())
        }

        #[sqlx::test]
        fn test_prepare_announcement_nonce_relationships(pool: PgPool) -> Result<()> {
            // Create a test oracle
            let db = DBconnection(pool);
            let oracle = create_test_oracle(&db)?;

            // Create a maturation time
            let maturation = Utc::now() + Duration::hours(1);

            // Prepare announcement
            let (announcement, sk_nonces) =
                oracle.prepare_announcement(maturation, &mut thread_rng())?;

            // Verify that public nonces match the secret nonces
            for (i, sk_nonce) in sk_nonces.iter().enumerate() {
                let oracle_r_kp =
                    secp256k1_zkp::Keypair::from_seckey_slice(&SECP, sk_nonce).unwrap();
                let expected_nonce = XOnlyPublicKey::from_keypair(&oracle_r_kp).0;
                assert_eq!(
                    announcement.oracle_event.oracle_nonces[i], expected_nonce,
                    "Public nonce at index {} doesn't match derived nonce from secret",
                    i
                );
            }

            Ok(())
        }

        #[sqlx::test]
        fn test_prepare_announcement_different_digit_counts(pool: PgPool) -> Result<()> {
            // Create a DB connection
            let db = DBconnection(pool);
            // Test with different digit counts
            let digit_counts = [1, 5, 10, 20, 32];
            let maturation = Utc::now() + Duration::hours(1);
            let mut rng = thread_rng();

            for &digits in &digit_counts {
                let oracle = create_test_oracle_with_digits(&db, digits)?;
                let (announcement, sk_nonces) =
                    oracle.prepare_announcement(maturation, &mut rng)?;

                // Verify the nonce counts match the digit count
                assert_eq!(
                    announcement.oracle_event.oracle_nonces.len(),
                    digits as usize,
                    "Nonce count should match digit count of {}",
                    digits
                );
                assert_eq!(
                    sk_nonces.len(),
                    digits as usize,
                    "Secret nonce count should match digit count of {}",
                    digits
                );
            }

            Ok(())
        }

        #[sqlx::test]
        fn test_prepare_announcement_variety_of_maturation_times(pool: PgPool) -> Result<()> {
            // Create a test oracle
            let db = DBconnection(pool);
            let oracle = create_test_oracle(&db)?;

            // Test with different maturation times
            let now = Utc::now();
            let maturation_times = [
                now,
                now + Duration::hours(1),
                now + Duration::days(1),
                now + Duration::days(30),
                now + Duration::days(365),
            ];

            for &maturation in &maturation_times {
                let (announcement, _) =
                    oracle.prepare_announcement(maturation, &mut thread_rng())?;

                // Verify the maturation time is set correctly
                assert_eq!(
                    announcement.oracle_event.event_maturity_epoch as i64,
                    maturation.timestamp(),
                    "Maturation epoch should match timestamp {}",
                    maturation
                );

                // Verify event ID includes the correct timestamp
                let expected_id = oracle.asset_pair_info.asset_pair.to_string().to_lowercase()
                    + maturation.timestamp().to_string().as_str();
                assert_eq!(
                    announcement.oracle_event.event_id,
                    expected_id,
                    "Event ID should include timestamp {}",
                    maturation.timestamp()
                );
            }

            Ok(())
        }

        #[sqlx::test]
        fn test_prepare_announcement_signature_verification(pool: PgPool) -> Result<()> {
            // Create a test oracle
            let db = DBconnection(pool);
            let oracle = create_test_oracle(&db)?;

            // Create a maturation time
            let maturation = Utc::now() + Duration::hours(1);

            // Prepare announcement
            let (announcement, _) = oracle.prepare_announcement(maturation, &mut thread_rng())?;

            // Verify the signature
            let message = Message::from_digest(
                sha256::Hash::hash(&announcement.oracle_event.encode()).to_byte_array(),
            );

            // The signature should verify with the oracle's public key
            SECP.verify_schnorr(
                &announcement.announcement_signature,
                &message,
                &announcement.oracle_public_key,
            )
            .expect("Signature verification should succeed");

            Ok(())
        }

        #[sqlx::test]
        fn test_prepare_announcement_deterministic_event_id(pool: PgPool) -> Result<()> {
            // Create a test oracle
            let db = DBconnection(pool); // For test, we don't need a real DB
            let oracle = create_test_oracle(&db)?;

            // Create a maturation time
            let maturation = Utc::now() + Duration::hours(1);

            // Prepare two announcements with the same parameters
            let mut rng = thread_rng();
            let (announcement1, _) = oracle.prepare_announcement(maturation, &mut rng)?;
            let (announcement2, _) = oracle.prepare_announcement(maturation, &mut rng)?;

            // The event IDs should be identical
            assert_eq!(
                announcement1.oracle_event.event_id, announcement2.oracle_event.event_id,
                "Event IDs should be deterministic for the same maturation time"
            );

            // But the nonces should be different (random)
            assert_ne!(
                announcement1.oracle_event.oracle_nonces, announcement2.oracle_event.oracle_nonces,
                "Nonces should be random between announcements"
            );

            Ok(())
        }
    }
}
