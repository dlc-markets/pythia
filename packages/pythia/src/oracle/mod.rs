use crate::{oracle::crypto::sign_outcome, AssetPairInfo};

use chrono::{DateTime, Utc};
use dlc_messages::oracle_msgs::{
    EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent,
};
use lightning::util::ser::Writeable;
use secp256k1_zkp::{
    rand::{thread_rng, RngCore},
    schnorr::Signature,
    All, KeyPair, Message, Secp256k1, XOnlyPublicKey,
};

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
    secp: Secp256k1<All>,
    keypair: KeyPair,
}

impl Oracle {
    /// Create a new instance of oracle for a numerical outcome given an lib secp256k1 context, a postgres DB connection and a keypair.
    pub fn new(
        asset_pair_info: AssetPairInfo,
        secp: Secp256k1<All>,
        db: DBconnection,
        keypair: KeyPair,
    ) -> Oracle {
        Oracle {
            asset_pair_info,
            db,
            secp,
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

    /// Create an oracle announcement that it will sign the price at given maturity instant
    pub async fn create_announcement(
        &self,
        maturation: DateTime<Utc>,
    ) -> Result<OracleAnnouncement> {
        let event_id = self.asset_pair_info.asset_pair.to_string().to_lowercase()
            + maturation.timestamp().to_string().as_str();
        if let Some(event) = self.db.get_event(&event_id).await? {
            info!(
                "Event {} already announced (should be possible only in debug mode or when restarted)",
                &event_id
            );
            return Ok(compute_announcement(self, event));
        }
        let event = &self.asset_pair_info.event_descriptor;
        let digits = event.nb_digits;
        let mut sk_nonces = Vec::with_capacity(digits.into());
        let mut nonces = Vec::with_capacity(digits.into());
        // Begin scope to ensure ThreadRng is drop at compile time so that Oracle derive Send AutoTrait
        '_rng_scope: {
            let mut rng = thread_rng();
            for _ in 0..digits {
                let mut sk_nonce = [0u8; 32];
                rng.fill_bytes(&mut sk_nonce);
                let oracle_r_kp =
                    secp256k1_zkp::KeyPair::from_seckey_slice(&self.secp, &sk_nonce).unwrap();
                let nonce = XOnlyPublicKey::from_keypair(&oracle_r_kp).0;
                sk_nonces.push(sk_nonce);
                nonces.push(nonce);
            }
        } // End scope: ThreadRng is drop at compile time so that Oracle derives Send AutoTrait

        let oracle_event = OracleEvent {
            oracle_nonces: nonces,
            event_maturity_epoch: maturation.timestamp() as u32,
            event_descriptor: EventDescriptor::DigitDecompositionEvent(
                self.asset_pair_info.event_descriptor.clone(),
            ),
            event_id,
        };

        let announcement = OracleAnnouncement {
            announcement_signature: self.secp.sign_schnorr(
                &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                    &oracle_event.encode(),
                ),
                &self.keypair,
            ),
            oracle_public_key: self.keypair.public_key().into(),
            oracle_event,
        };
        self.db
            .insert_announcement(&announcement, sk_nonces)
            .await?;
        debug!("created oracle announcement with maturation {}", maturation);

        trace!("announcement {:#?}", &announcement);

        Ok(announcement)
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
                &event_id
            );
            return Ok(compute_attestation(self, event));
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
            .iter()
            .zip(outstanding_sk_nonces.iter())
            .map(|(outcome, outstanding_sk_nonce)| {
                sign_outcome(&self.secp, &self.keypair, outcome, outstanding_sk_nonce)
            })
            .unzip();

        let attestation = OracleAttestation {
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes: outcome_vec,
        };
        debug!(
            "created oracle attestation with maturation {}",
            event.maturity
        );
        trace!("attestation {:#?}", &attestation);

        let _ = &self
            .db
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
        Ok(Some((announcement, compute_attestation(self, event))))
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
        let event_id =
            ("btcusd".to_string() + &maturation.timestamp().to_string()).into_boxed_str();
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
                            let oracle_r_kp =
                                secp256k1_zkp::KeyPair::from_seckey_slice(&self.secp, sk)
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
                            secp256k1_zkp::KeyPair::from_seckey_slice(&self.secp, &sk_nonce)
                                .unwrap();
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
            announcement_signature: self.secp.sign_schnorr(
                &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                    &oracle_event.encode(),
                ),
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
            .iter()
            .zip(sk_nonces.iter())
            .map(|(outcome, outstanding_sk_nonce)| {
                sign_outcome(&self.secp, &self.keypair, outcome, outstanding_sk_nonce)
            })
            .unzip();

        let attestation = OracleAttestation {
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

fn compute_attestation(oracle: &Oracle, event: PostgresResponse) -> Option<OracleAttestation> {
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
    use std::{str::FromStr, time::Duration};

    use chrono::{DateTime, DurationRound, SubsecRound, Utc};
    use dlc_messages::oracle_msgs::{
        DigitDecompositionEventDescriptor, EventDescriptor, OracleAnnouncement, OracleAttestation,
        OracleEvent,
    };
    use lightning::util::ser::Writeable;
    use secp256k1_zkp::{rand, schnorr::Signature, KeyPair, Message, Secp256k1, XOnlyPublicKey};
    use sqlx::postgres::PgPool;

    use super::{
        crypto::test::{check_signature_with_nonce, check_signature_with_tag},
        error::OracleError,
        postgres::DBconnection,
        Oracle,
    };
    use crate::{
        config::{AssetPair, AssetPairInfo},
        pricefeeds::{
            error::PriceFeedError,
            ImplementedPriceFeed::{self, Lnmarkets},
        },
    };

    use secp256k1_zkp::hashes::Hash;

    async fn setup_oracle(
        tbd: PgPool,
        precision: i32,
        nb_digits: u16,
        pricefeed: ImplementedPriceFeed,
    ) -> Oracle {
        let asset_pair = AssetPair::BtcUsd;
        let event_descriptor = DigitDecompositionEventDescriptor {
            base: 2,
            is_signed: false,
            unit: "btc/usd".into(),
            precision,
            nb_digits,
        };
        let asset_pair_info = AssetPairInfo {
            pricefeed,
            asset_pair,
            event_descriptor,
        };

        let secp = Secp256k1::new();
        let (secret_key, _) = secp.generate_keypair(&mut rand::thread_rng());
        let keypair = KeyPair::from_secret_key(&secp, &secret_key);
        let db = DBconnection(tbd);
        return Oracle::new(asset_pair_info, secp, db, keypair);
    }

    #[sqlx::test]
    async fn test_oracle_setup(tbd: PgPool) {
        let oracle = setup_oracle(tbd.clone(), 0, 20, Lnmarkets).await;
        let event = oracle.asset_pair_info.clone().event_descriptor;
        assert_eq!((0, 20), (event.precision, event.nb_digits));
        let oracle = setup_oracle(tbd, 10, 20, Lnmarkets).await;
        let event = oracle.asset_pair_info.clone().event_descriptor;
        assert_eq!((10, 20), (event.precision, event.nb_digits))
    }

    async fn test_announcement(oracle: &Oracle, date: DateTime<Utc>) {
        let oracle_announcement = oracle.create_announcement(date).await.unwrap();

        let db_oracle_announcement = oracle
            .oracle_state(&("btc_usd".to_owned() + date.timestamp().to_string().as_str()))
            .await
            .inspect_err(|e| println!("{}", e))
            .unwrap()
            .unwrap_or_else(|| panic!("No oracle announcement in DB !"));
        assert_eq!(oracle_announcement, db_oracle_announcement.0);

        let secp = &oracle.secp;
        secp.verify_schnorr(
            &oracle_announcement.announcement_signature,
            &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                oracle_announcement.oracle_event.encode().as_ref(),
            ),
            &oracle.get_public_key(),
        )
        .unwrap();
    }

    #[sqlx::test]
    async fn announcements_tests(tbd: PgPool) {
        let oracle = setup_oracle(tbd, 12, 32, Lnmarkets).await;
        let now = Utc::now();
        let dates = [60, 3600, 24 * 3600, 7 * 24 * 3600]
            .iter()
            .map(|t| now - Duration::new(*t, 0))
            .chain(
                [
                    DateTime::parse_from_rfc3339("2022-01-01T00:00:00+11:00")
                        .unwrap()
                        .with_timezone(&Utc),
                    DateTime::parse_from_rfc3339("2023-01-01T00:00:00+11:00")
                        .unwrap()
                        .with_timezone(&Utc),
                ]
                .into_iter(),
            );
        for date in dates {
            test_announcement(&oracle, date).await;
        }
    }

    async fn test_attestation(oracle: &Oracle, date: DateTime<Utc>) {
        let oracle_announcement = oracle.create_announcement(date.clone()).await.unwrap();
        let now = Utc::now();
        let oracle_attestation = match oracle
            .try_attest_event(&oracle_announcement.oracle_event.event_id)
            .await
        {
            Ok(attestation) => attestation,
            Err(OracleError::PriceFeedError(error)) => {
                let PriceFeedError::PriceNotAvailable(_, asked_date) = error else {
                    panic!(
                        "Pricefeed {:?} did not respond for this date {}",
                        oracle.asset_pair_info.pricefeed,
                        date.clone()
                    )
                };
                // Pricefeed can only respond that price is not available if our query was asking in the future
                if asked_date < now {
                    panic!("Pricefeed {:?} say price is not available for {}, which is not in the future (now it is: {}). Maybe only recent index are available.", oracle.asset_pair_info.pricefeed, asked_date, now)
                };
                return;
            }
            _ => panic!("DB error"),
        };
        assert_eq!(
            oracle_attestation,
            oracle
                .oracle_state(&oracle_announcement.oracle_event.event_id)
                .await
                .unwrap()
                .unwrap()
                .1
        );
        match oracle_attestation {
            None => assert!(
                oracle_announcement.oracle_event.event_maturity_epoch > now.timestamp() as u32
            ),
            Some(attestation) => {
                let event = &oracle.asset_pair_info.event_descriptor;
                let precision = event.precision;
                assert!(
                    (oracle
                        .asset_pair_info
                        .pricefeed
                        .get_pricefeed()
                        .retrieve_price(AssetPair::BtcUsd, date)
                        .await
                        .unwrap()
                        - attestation
                            .outcomes
                            .iter()
                            .rev()
                            .enumerate()
                            .map(|c| c.1.parse::<f64>().unwrap()
                                * (2_f64.powf(c.0 as f64 - (precision as f64))))
                            .sum::<f64>())
                    .abs()
                        < 0.01
                );
                let secp = &oracle.secp;
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

    #[sqlx::test]
    async fn attestations_test(tbd: PgPool) {
        let oracle = setup_oracle(tbd, 12, 32, Lnmarkets).await;
        let now = Utc::now()
            .trunc_subsecs(0)
            .duration_trunc(chrono::Duration::minutes(1))
            .unwrap();
        let dates = [60, 3600, 24 * 3600, 7 * 24 * 3600, 30 * 24 * 3600]
            .iter()
            .map(|t| now - Duration::new(*t, 0))
            .chain([now + Duration::new(180, 0)].into_iter());
        for date in dates {
            test_attestation(&oracle, date).await;
        }
    }

    fn check_test_vec(attestation_test_vec: OracleAttestation) {
        let secp = Secp256k1::new();
        attestation_test_vec
            .signatures
            .into_iter()
            .zip(&attestation_test_vec.outcomes)
            .map(|(sig, dig)| {
                (
                    sig.clone(),
                    check_signature_with_nonce(
                        &secp,
                        &attestation_test_vec.oracle_public_key,
                        dig,
                        None,
                        sig,
                    ),
                )
            })
            .for_each(|res| {
                if let Err(e) = res.1 {
                    println!("Error: {:?}", e);
                    panic!("{:?} is invalid", res.0)
                }
            });
    }

    #[test]
    fn test_vector_announcement() {
        // https://oracle.holzeis.me/asset/btcusd/announcement/2023-06-16T13:00:00Z
        let digit_decomposition = DigitDecompositionEventDescriptor {
            base: 2,
            is_signed: false,
            unit: "usd/btc".into(),
            precision: 0,
            nb_digits: 20,
        };
        let oracle_event = OracleEvent {
            event_id: "btcusd1686920400".into(),
            event_maturity_epoch: 1686920400,
            event_descriptor: EventDescriptor::DigitDecompositionEvent(digit_decomposition),
            oracle_nonces: [
                "5ff9c4efb62b88f2538e9e5702240b5043a94e79e584fca308e96df2a31ae370",
                "67ed40f3583174d40324272b4ec7c5d895b1c50bc0738709819803db3370d9bd",
                "c349532d57fd1fc2e17660312cd8ab641babc7953b1f0d1c1a984a095ff4e499",
                "64e07de1a613e65efc629f6a81ae743931e6edd2fd2c872e8411d5edb1297428",
                "348a792836b63a9b978a24825f94f84fda30eb969e1e25dc991dbe1ff5009a39",
                "9b0a54dcec9b1aba03f975989dbc85b5bba9c7ebedc6e707e382aebf67095527",
                "83f02c21a93c11310724f3e11683b1967f580ce78f82edef4383c85fed01821c",
                "b0d7e95c2c906b6b1668208dfc4361f4f207581b79df58ca628cbbba1bbba2b2",
                "ae46c8713545052b8df6a2b51a7fcf39f899390815375d2c05b8b34272f5b429",
                "04fc6e5a9ac57dd8dd3caf16f415dd8bf16ae869a1d99d33720f45d33c4c3220",
                "9a95abc04114624bfb9a984c8bc47b226be4263e3a26726c5d2e2574a3b48ac7",
                "a01d61415732e4a8783981690ba337bb11631f52c1f46ffe3fe9a83f909d0a57",
                "a04b80ed964d1a837a56d6a19247357ed854adde6a181a3a3601c24618788657",
                "64858bd9c7bebfad02e7f9e266cfe9c55bf2ba381f8639c28773c5e87dfff1bb",
                "7b09fe7fe76c474f4a5ed9192f05a3557040020d9180b43e390ed888b2259389",
                "1aab06df357c01836ae84437ed9d92098f7e5620f5b88e42a02f22cf1fda748b",
                "03b5f4adcbc5eee6f12125004f2a0658ec5d63ce778c284461494581c9f827fe",
                "15e5873f37c2aa548194c11ef6b384501e38c17e00dfdc1a98cb7f079373745c",
                "f85a83b4cc7575218ba12fcfa8c0e7bf9fb4c0eff0afc10bbd977620bbace87d",
                "6b9953c3c05b4dae9018c9c4a2318658396a19a53f6fe3a4498cab44a9727058",
            ]
            .into_iter()
            .map(|s| XOnlyPublicKey::from_str(s).unwrap())
            .collect(),
        };
        let announcement = OracleAnnouncement {
            announcement_signature: Signature::from_str("4aefe5864eeffffae95b905e6b4e0cfdab99b6e1aed8a3ae873b250708104f13aa46e725d292b6abf5299c6decdf9436762ec76f6399de9a03074f3c93ef1c08").unwrap(), 
            oracle_public_key: XOnlyPublicKey::from_str("16f88cf7d21e6c0f46bcbc983a4e3b19726c6c98858cc31c83551a88fde171c0").unwrap(), 
            oracle_event
        };
        let secp = Secp256k1::new();
        secp.verify_schnorr(
            &announcement.announcement_signature,
            &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                announcement.oracle_event.encode().as_slice(),
            ),
            &announcement.oracle_public_key,
        )
        .unwrap();
    }

    #[test]
    fn tests_vector_attestation() {
        // https://oracle.holzeis.me/asset/btcusd/attestation/2023-06-16T13:00:00Z
        let attestation_test_vec = OracleAttestation {
            oracle_public_key: XOnlyPublicKey::from_str("16f88cf7d21e6c0f46bcbc983a4e3b19726c6c98858cc31c83551a88fde171c0").unwrap(),
            signatures: [
                "5ff9c4efb62b88f2538e9e5702240b5043a94e79e584fca308e96df2a31ae37064712a2f4a568c9a7e602c8dd862ba450570a1bc4dc9143049aa27d8a58872c0",
                "67ed40f3583174d40324272b4ec7c5d895b1c50bc0738709819803db3370d9bdf9e8c534d639f9b5f26974b827dfb95d53711bbe7376daede29efa5ce3a9fffa",
                "c349532d57fd1fc2e17660312cd8ab641babc7953b1f0d1c1a984a095ff4e499b7853faf6a09cebafc31a3a77cef84b6dc1aff8cf3d777ad7b6d250f985e3c73",
                "64e07de1a613e65efc629f6a81ae743931e6edd2fd2c872e8411d5edb12974288b24c829cbf8a61a689f48bb7d7ca45158083e612808993c11cd993ef6d6bc68",
                "348a792836b63a9b978a24825f94f84fda30eb969e1e25dc991dbe1ff5009a39a1bc021de3dea59bd92bbfa700742bcf84221d0204e6467dd7635df9684ebfe1",
                "9b0a54dcec9b1aba03f975989dbc85b5bba9c7ebedc6e707e382aebf6709552761ce17a6c43630fa11bcf63ac96eb8f3bf24df8f46378366afc5e897960618cf",
                "83f02c21a93c11310724f3e11683b1967f580ce78f82edef4383c85fed01821c80aaff23213b64b7400b7890fe6c065ce3cedb5c5e166cccbe692db30c91f572",
                "b0d7e95c2c906b6b1668208dfc4361f4f207581b79df58ca628cbbba1bbba2b2942533083e0eeb8ceeba4caff8fcc6ca716cc9c595fa483e68f6a7c73f742666",
                "ae46c8713545052b8df6a2b51a7fcf39f899390815375d2c05b8b34272f5b429b2fb13a5ac906b6874e806d5aa84fe1bf4caf5c6402ae8eb5c2b7498f8024e41",
                "04fc6e5a9ac57dd8dd3caf16f415dd8bf16ae869a1d99d33720f45d33c4c322068b9d6ddd1e95eac60402410bbf5a071feb2f2233694e9f1141bfc2ba8776147",
                "9a95abc04114624bfb9a984c8bc47b226be4263e3a26726c5d2e2574a3b48ac745c58a58090a417d5a8c4697ab74ecfe0a93a80cd420a8a9a8d9d79d6728e447",
                "a01d61415732e4a8783981690ba337bb11631f52c1f46ffe3fe9a83f909d0a5769bd4d9122220fba7bbb7745d784e9aab42e5f301218a42dbb097688f0b8c6aa",
                "a04b80ed964d1a837a56d6a19247357ed854adde6a181a3a3601c2461878865725ec6d0317630aebdb80fa74e4e9a1879533b025a802950cba932d238899ec40",
                "64858bd9c7bebfad02e7f9e266cfe9c55bf2ba381f8639c28773c5e87dfff1bb157df7968fd234fb21421083aeb4633019bdd1d6f6473a86b77f18f762f45902",
                "7b09fe7fe76c474f4a5ed9192f05a3557040020d9180b43e390ed888b22593896d547c51e8d1f9dadfdeec36a46785063062192ca665d5e7d57f67124800478d",
                "1aab06df357c01836ae84437ed9d92098f7e5620f5b88e42a02f22cf1fda748b8dff36e60d5741e2ceb60cd86a6dbb0ee453ce3b0c56b5c6dbf78f193faa17dd",
                "03b5f4adcbc5eee6f12125004f2a0658ec5d63ce778c284461494581c9f827fe7e9d18554af34abffdec193df8b20a9d99e263f95e093a39bf1f25f31608f11d",
                "15e5873f37c2aa548194c11ef6b384501e38c17e00dfdc1a98cb7f079373745c975042a0d57ed2692221305227afad80a273c2afc1e1c68332e4326485de1286",
                "f85a83b4cc7575218ba12fcfa8c0e7bf9fb4c0eff0afc10bbd977620bbace87d9f673fc41d7927ae6aadae3edd218acb7f2a7bad15c7e72a068f0b0166971b83",
                "6b9953c3c05b4dae9018c9c4a2318658396a19a53f6fe3a4498cab44a9727058782d2747a0978674fd8da03a8b09610edcb46d374b7acbe15aa39adc4ab56d46"
                ]
            .into_iter().map(|s| Signature::from_str(s).unwrap()).collect(),
            outcomes: [
                "0",
                "0",
                "0",
                "0",
                "0",
                "1",
                "1",
                "0",
                "0",
                "0",
                "1",
                "1",
                "1",
                "1",
                "0",
                "0",
                "1",
                "1",
                "1",
                "0"
                ].into_iter().map(|d| d.to_string()).collect()
        };
        check_test_vec(attestation_test_vec);
        // https://oracle.p2pderivatives.io/asset/btcusd/attestation/2023-07-06T16:20:00Z
        let attestation_test_vec = OracleAttestation {
            oracle_public_key: XOnlyPublicKey::from_str("ce4b7ad2b45de01f0897aa716f67b4c2f596e54506431e693f898712fe7e9bf3").unwrap(),
            signatures: [
                "9c4b0f2f79a7b6baa152274fedaa8576afaf912552057d01487dac2d11ca93487a53f91ddbfac44da9a828f42a1287f8f0290113d895616bc6f8ea3fbb78c623",
                "53c04155a29eba213da519e1444beb6f9ee86fb3e081eba15c149a4816065603e031f1c0c3cbf163449df5bcc83b5995c58ad27adb36a02e850ddc8854d55ed1",
                "1c303f270c78fcbae632b9b5d76e29d37ed9db3be826829ce63e95f306aa68b15d63b40bb89e85f4183584c805600639b1758da17ebc61d53d2483d0bd4411f2",
                "ae19f4437756fddb9561839503fb9dec4f2de1dac3b89a342e4f4c2f19122c938f62e90b396c5102e11113110e6d75aaac15dc2a818bd4c2e6a4f471bd4d19a7",
                "b386f6ed874b9f852edf1746bf668812853872da394d56fbe79aa722c8919052f352322537daed6665ff831bd00944d9e4351c61db1ce30c35fd7a8a513e84da",
                "2c98a65da7da0780d6392b0f59a3e38605ce29cc2312239980442296741f722acaad32ff5434f1fda1e5eacef390f2758c5ae6f169117b2ff5b14c9738bd295c",
                "fbce242431acf3a3e6af84844b64491700d89cddca8f8009ccfbf2529edb6b57226410f4e804199ec532408b7805f3418d39b6cb458fd0e4a15fe830e8b2d1d8",
                "db25e234b6abe9dd54fdb432f0268c7cecbf2149d1f2e4a051b1842ba5e26bbc325143a1a3c44f76427239bf4d3ae01cebca2566398eb3b528b318264eb60bbd",
                "d03216bbd03a4f82749cda95f362f76e8b10f7f077d3a18d6a7923530aaa63bfa93f7f50a8e69502f8fb58c03fb173f5e00a48b7263be89cb83f8ca89ff10263",
                "c6c295aeda63b7d6302861c5c1c15afe446ba59d2a7db2e7b7eaceee7bc7d9aea3c740f21fab1f28d5b4f1c856d617c3a0f462bb286a930d8d5772cfaaa979f6",
                "fb19c2ad97d13088dd65b50cb59facfdc23785257743119f4debc2b2c1c09f18fa01a01d51393063d7256c073c0de7203675ab59d7d38d9b1ea4b91eadad14fd",
                "3ebe4cc0b838c90febee005ea54546b47946f39a02ff8871592e40d4cfe8b2e44aa09da1cc6fd775e5c9eb0f11657a881f05e4f109bf5e0d9e9085a3c92366d4",
                "ff072e47f4781fd54133898a00e2bab74a9497c09189d79ac1002bf452ac81e3f6ca7867cda36f90c655cab61c68638b5a2c932574296d2e4a6ca98f3102b601",
                "3eb013084033d026e5cb5724dec0e1c6a27a2f0c90340b42f8a0f3a6547e26220ada431b88f428d961f2a55b50a27965ea9356d663034eb58700bdfc17312dd6",
                "627c955540825e9f3a9920e432fe1ee6a4624c65cb2d7dccdd47632d6047f962d9462b2d59029812e34bd472f0541cba9f5538b3c1bc686d40d4a2238943b845",
                "7c9474ca70948cad743300cb9ca9e8657fbc7b24713efda0d761b889e3896f834b3b7c9c9738b2c624e95106cef30ce679265cd8767078f6ca26afb18c13f738",
                "5dd67b7f10d2c94b183c19c6089865487204f031ecd648691b8098e5340e3313d78313457af73b6aa7732da5e8e43bf23740795bc3641ef2e569705b3ec8e75c",
                "7b22498833b338f7a37ce23f0ddb8600aab3e26c5cf20a51471c417864d96fe99043bc90c8a81e2e5a909d2a65bd827509c521883cceeefdd45f8ca33f7ac92c",
                "702e4c56f6bda3e8ce41726eece59cc179f81cb18dadf081f710a1921e433cab28c27c98b736f558188edb42398683315d9162f5825ee004e569c7138d086357",
                "b3561f1f2f20b89cc453772695ec7b1d044d7f8aa48cd56ab771f952266688fca7336ecb3edfa080d2b621ac80604a21087bc14fe47bb6a2f3bca2227638fd69"
                ]
            .into_iter().map(|s| Signature::from_str(s).unwrap()).collect(),
            outcomes: [
                "0",
                "0",
                "0",
                "0",
                "0",
                "1",
                "1",
                "1",
                "0",
                "1",
                "1",
                "0",
                "1",
                "0",
                "1",
                "0",
                "0",
                "0",
                "0",
                "0"
                ].into_iter().map(|d| d.to_string()).collect()
        };
        check_test_vec(attestation_test_vec);
        //http://localhost:8000/v1/asset/btcusd/attestation/2023-07-07T10:48:00Z
        let attestation_test_vec = OracleAttestation {
            oracle_public_key: XOnlyPublicKey::from_str("24653eac434488002cc06bbfb7f10fe18991e35f9fe4302dbea6d2353dc0ab1c").unwrap(),
            signatures: [
                "cfade64b475a23bcbea453ad17349a3907683c1aa09bcff4d4c319f6444adf9f6ac38e0c60bcd8c14c91faaec429e72803a870e8e29a5ae0fe2392e874dd9b08",
                "0831992d36b965a137c9b6ac1fed802b2d7f7532ed25b5e82918877611017eb0e5fcb3eb41d1cc8b7a65375d340608e092b3c4e0d858f45124f32accb2a5242c",
                "46a38d9d5db1be0d5ecc16d5798433acc35218ed57be0b0d32dc37902abd0d30b85a74d81f7197345f78f081f53c5f1087a7731f868acce80a237a3a4e1fa042",
                "9e33a3cf215d519b0e76c4f705b8dc101bb27f267e32cad2a721d44c9ff05f3f0312c7ea68f1947468f2eb104d010c8a243872e14ce507988c14670fc19c4a53",
                "49c8ff407c96e936dca6c43be81c26c6dd246ce866ba0ceb3f794fa69c31b8753cd0366cd9359df89bb5792c3f5a0b526e306e8b863945ba0a99fc9f2a778533",
                "d0e008ce607d22b35dcbe9b3b507acbba52b99a08c987a3820ac4cc89e0d78683bd40964502689e36f8235219e87965a65f0fe80d671893a210de4b699ebc5fa",
                "dfac958180cffbf59e607059ccc5573c537ecdde2602196e9319c9ffa5536ec556d200e11ee26cf8b03c0d214b705957a3758eaf05b8ec3cee254396a22a4126",
                "b6f61b4893911c4205d4f2c18f73ea54c0fafd2c4f952018af7f30d6e2276f329fdb45e551b8bebda232b0d068f659bed131fc03e8f4b9ae123cc33cde11e7b8",
                "e06ab8906619fad0036ea6babbcf61602bd522451300d44a5a56801684699cc7ec6b5b0bb1c33e1547551ee36d2b845e70fe5192b9481e2fc1c63a3837578346",
                "4dc1281ddfca3194b6edd706047d355163a061e7b5044ae2369f412ff4f68ec6a02f9e4bac5280c1de8ed8300e5281c084deb8c11374f007aa6584766d632cdd",
                "3c18ea01a733642cf8b1296784508faf2b2c6a4e0add1021c954566936133dc7e62a2997a67a9ab2d6251973938fa76ba5d7dfa7e9b06e20666b475f0484b2be",
                "e880892c32a3f3826c43933146e172406e70f1011bfd12fe1ea5e831fe909b4c5bbb0b45229707d0bf71891b66787b75643fd0b03183d6dc0baf515091e76fa0",
                "e4c1175a76b165042a497942d4710401fb5e67a8be48c873addc5cb47334024f0d1b9630055cfe37e35ed7dddc72530ab8a389449860186890a3bf498d573a4a",
                "48b2e82bed1516c59fb280b9755705b2b125876ed1e10b8677dbe89bec8745700726535c3f3821edc7d85b195167d0e0c9dc06d40c6790eada760fc5e7bc3ce6",
                "6ecac130bca31ae9ca4a8562dcb66610da92bc2caaef29a30eb75bfaf4e5057377579123829a08a8c24ad0ec30be571cb36a720fff433fdceb064373fe52dae1",
                "715c8fbf27df525b42601152f626aa4bd2290dfcf09510485d68a93d17e8c863256f878b60548acb3404c3836302675c5503e5e5db3751157c0648c999f3ecde",
                "96c80f7649c7d6c2d091c06d5b31bf61ca6d537d4eea24c074f6e85b1c74838c1adce1ee0e1e6cf57a5832fa5b70e6f661b556927ddf297acea1db5879c47302",
                "521cd8c6e689695a6ec214c626d215479abcd17464c5e864f320205c9fdace3a60f05f74ae04f8609be975fead526fb3409dfa79506e9d675eebae4563aefc78",
                "029c874c6c471b0611e1ba45cfe337a3cdf4b4c6c57e89e068e4e6d9d6473a33a3591fa144605eab1bd05d670fe064cd754b7d10ed94ef9e866b625c3ba5712a",
                "1254eb01919f7f90469d38f9a03d4083631b30e55086701a080c0df20dc20dcd19089a3a48a06650580e3fa744434d82bf73663147523d5850d704e04ecdbd4d"
                ]
            .into_iter().map(|s| Signature::from_str(s).unwrap()).collect(),
            outcomes: [
                "0",
                "0",
                "0",
                "0",
                "0",
                "1",
                "1",
                "1",
                "0",
                "1",
                "1",
                "0",
                "0",
                "1",
                "0",
                "0",
                "1",
                "1",
                "0",
                "1"
                ].into_iter().map(|d| d.to_string()).collect()
        };
        check_test_vec(attestation_test_vec);
    }

    fn check_test_vec_dlc_specs(attestation_test_vec: OracleAttestation) {
        let secp = Secp256k1::new();
        attestation_test_vec
            .signatures
            .into_iter()
            .zip(attestation_test_vec.outcomes)
            .map(|(sig, dig)| {
                (
                    sig.clone(),
                    check_signature_with_tag(
                        &secp,
                        &attestation_test_vec.oracle_public_key,
                        dig,
                        "DLC/oracle/attestation/v0",
                        sig,
                    ),
                )
            })
            .for_each(|res| {
                if let Err(e) = res.1 {
                    println!("Error: {:?}", e);
                    panic!("{:?} is invalid", res.0)
                }
            });
    }

    #[test]
    #[should_panic] // Surprising because we should have follow suredbits spec
    fn test_vector_announcement_suredbits() {
        // DOES NOT PASS ? HASH TAG OR TLV SERIALIZATION IS INCORRECT ?
        // SUREDBITS AND 10101 DO NOT FOLLOW THE SAME SPECIFICATION WE FOLLOW 10101 SPEC FROM RUST DLC

        // https://oracle.suredbits.com/announcement/3ef91d749960d85f1190e86bd89d7d65a6303ca9bd4f16111c569d46d81f8f04
        let digit_decomposition = DigitDecompositionEventDescriptor {
            base: 2,
            is_signed: false,
            unit: "BTCUSD".into(),
            precision: 0,
            nb_digits: 18,
        };
        let oracle_event = OracleEvent {
            event_id: "Deribit-BTC-10APR23".into(),
            event_maturity_epoch: 1681106400,
            event_descriptor: EventDescriptor::DigitDecompositionEvent(digit_decomposition),
            oracle_nonces: [
                "02ef69281933974a279c4961adc96a442847739bd4af2d137fcac924438a223e",
                "1fdc6fcb01764771c4f6edd52aebb0cfc069953c83dddf9e8e03ba316e2799e0",
                "3dbf7826a18233f7b214a1695a0453aaa36665a907d72544ccc3f73686e09598",
                "3e62346e411fbb6686eeb2e35f9cea155105aa40fef6c06680b672fda38a065b",
                "6a0839f21b194b785de639d22eb31441511e687e2a8c1d5d6ba00a3757a783b8",
                "7d5a3e5a160b0d71349fb6baef76cffbfff4dcf512564edbb7b6c71cd334fd28",
                "802d35ad5fe58a16852dbb9ca461d247415ceb340e30076b0c4e5b67ad87dd39",
                "837731cc31462753c6f6431248e7c50223fa40905ae009711efe676007399ef3",
                "963b3c8450b880e517861a3511e54dea2539206450327cfd03fd07a456e21d6a",
                "96597ceffd09c8465ec8dc2175a333b9b8ea9e7a0d560f171ca72883392683bf",
                "9a9a35ea727549c1ee00e60059fc07c6bae2fa185d75d9d4aa9339c14d96d7ef",
                "a6c76aa37f009e0be9a327fd2ca8ed211e10a7f88bdaf61fee01cff2cb47e8c0",
                "c5b3474e1f2cef7cd95d083f3552848f23948255f569ca8c4077c3dc2852b18a",
                "d257ede428866f9dd4aa27e942afc1941e50cdf90728757de4c18a17816a4850",
                "d59f8b04317a3804668126293e985583457a47fcf27658217fe9018531a87c34",
                "dc511785d266148d215c096df7a0e71a6e7f54be2ef5ac2da435f7ddf9c27f2a",
                "e16fe2dadb562db45ca3deaafc6917487a50c558cc1d8ca608a87163d6356ff5",
                "f31a6e3680e6142487754d1ba6790949e6f2d9ac1644036cdfae5edac773ba7e",
            ]
            .into_iter()
            .map(|s| XOnlyPublicKey::from_str(s).unwrap())
            .collect(),
        };
        let announcement = OracleAnnouncement {
            announcement_signature: Signature::from_str("2a01eb079bac560e9496c66cfa21d2baf5607d7d78eb1682de699319433f902ba3e5023d309fa9253c7333d861c6ca78270083f4f970058a8a0ae81f7e4059fd").unwrap(),
            oracle_public_key: XOnlyPublicKey::from_str("04ba9838623f02c940d20d7b185d410178cff7990c7fcf19186c7f58c7c4b8de").unwrap(),
            oracle_event
        };
        let secp = Secp256k1::new();
        // check_signature_with_tag(
        //     &secp,
        //     &announcement.oracle_public_key,
        //     announcement.oracle_event.encode(),
        //     "DLC/oracle/announcement/v0",
        //     announcement.announcement_signature,
        // )
        // .unwrap();
        let tag_hash =
            <secp256k1_zkp::hashes::sha256::Hash>::hash("DLC/oracle/announcement/v0".as_bytes());
        let tag = tag_hash.as_ref();
        let payload = [tag, tag, announcement.oracle_event.encode().as_slice()].concat();
        secp.verify_schnorr(
            &announcement.announcement_signature,
            &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(payload.as_ref()),
            &announcement.oracle_public_key,
        )
        .unwrap();
    }

    #[test]
    fn tests_vector_attestation_suredbits() {
        // SUREDBITS AND 10101 DO NOT FOLLOW THE SAME SPECIFICATION WE FOLLOW 10101 SPEC FROM RUST DLC
        // USE TAGGED HASHES INSTEAD

        // https://oracle.suredbits.com/announcement/3ef91d749960d85f1190e86bd89d7d65a6303ca9bd4f16111c569d46d81f8f04
        let attestation_test_vec = OracleAttestation {
            oracle_public_key: XOnlyPublicKey::from_str("04ba9838623f02c940d20d7b185d410178cff7990c7fcf19186c7f58c7c4b8de").unwrap(),
            signatures: [
                "02ef69281933974a279c4961adc96a442847739bd4af2d137fcac924438a223e0dd7103814689c706241f789a1f48a6670d24045b874e6956b0e673cc0160ecd",
                "1fdc6fcb01764771c4f6edd52aebb0cfc069953c83dddf9e8e03ba316e2799e0f00fd65a3f6358703e8dbf65598b105b895511fff6f64a10338df74feafbdf85",
                "3dbf7826a18233f7b214a1695a0453aaa36665a907d72544ccc3f73686e0959895e41c169d845f1c426b9a3453ca685c74734ed810f82ed3dc40744889f84683",
                "3e62346e411fbb6686eeb2e35f9cea155105aa40fef6c06680b672fda38a065bda4872e365b8caf15970fe98583c8d8009c9235ffdbcf8c53a930bde0178f8d8",
                "6a0839f21b194b785de639d22eb31441511e687e2a8c1d5d6ba00a3757a783b810a2af9ea505155fc516e9a6b64b62cbab3c19f0f8fad0dccce3fc23d7b716c6",
                "7d5a3e5a160b0d71349fb6baef76cffbfff4dcf512564edbb7b6c71cd334fd2882935abfd71e4d7754867addd8ef2a6d65ae2cf656c657bc3c8a04248c51ee48",
                "802d35ad5fe58a16852dbb9ca461d247415ceb340e30076b0c4e5b67ad87dd3917adcd183bf70495701017cd5261056ced452509235c179a5fff15f51b743f5b",
                "837731cc31462753c6f6431248e7c50223fa40905ae009711efe676007399ef35ed67e497289844059006763477db88a29ad440050cfe3ca2ee6be51b4494bd3",
                "963b3c8450b880e517861a3511e54dea2539206450327cfd03fd07a456e21d6a40478ea41699b491a2f9ebfd96a110ff1647ccc4708cdddd9922f1db5ec17548",
                "96597ceffd09c8465ec8dc2175a333b9b8ea9e7a0d560f171ca72883392683bf237433f303de77a19f6918a95419da6d231b2006e0d4b5dea7aee3e7aae70b87",
                "9a9a35ea727549c1ee00e60059fc07c6bae2fa185d75d9d4aa9339c14d96d7ef5cb78c1381958fb8293ca725077f9104a8342506bd34f4f3d8161a439338b3a7",
                "a6c76aa37f009e0be9a327fd2ca8ed211e10a7f88bdaf61fee01cff2cb47e8c05bb54a301e98d0d8b3b05471abf211f8280fb3b5b31bd2dfdaa1792fc70c9727",
                "c5b3474e1f2cef7cd95d083f3552848f23948255f569ca8c4077c3dc2852b18a552c9f6f825ed015dcca74b958ffc63eb0bcbe0cee8f42c7de17af2221424ae5",
                "d257ede428866f9dd4aa27e942afc1941e50cdf90728757de4c18a17816a4850ddf9ab9ad0a4acda2b8a7942b12d28184f18cab50c9fc2f2ad58c278dbef38d0",
                "d59f8b04317a3804668126293e985583457a47fcf27658217fe9018531a87c343f89e0fc722dc879248cfa9140f5820292a194240c4f7a768c42dfed7279af55",
                "dc511785d266148d215c096df7a0e71a6e7f54be2ef5ac2da435f7ddf9c27f2aad5fd6455c950d169dbbfe271c1b8cc39c7bc698ec58d95e60309eab6c6f10bf",
                "e16fe2dadb562db45ca3deaafc6917487a50c558cc1d8ca608a87163d6356ff565559301b79d753f226d806d87ad27df7711c99cafb39989ed28a3fe7becbcd1",
                "f31a6e3680e6142487754d1ba6790949e6f2d9ac1644036cdfae5edac773ba7e60d053288c14e93f280f8f0f21dd625950ac64ef1faa74997191368d9b742f85"
                ].into_iter().map(|s| Signature::from_str(s).unwrap()).collect(),
            outcomes: [
                "0",
                "0",
                "0",
                "1",
                "1",
                "0",
                "1",
                "1",
                "1",
                "0",
                "0",
                "1",
                "1",
                "1",
                "1",
                "1",
                "1",
                "1"
                ].into_iter().map(|d| d.to_string()).collect(),
            };

        check_test_vec_dlc_specs(attestation_test_vec);
    }
}
