use crate::{oracle::crypto::sign_outcome, AssetPairInfo};

use dlc_messages::oracle_msgs::{
    EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent, Writeable,
};
use secp256k1_zkp::{
    rand::{thread_rng, RngCore},
    All, KeyPair, Message, Secp256k1, XOnlyPublicKey,
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
pub use crypto::*;
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
    pub fn get_public_key(self: &Oracle) -> XOnlyPublicKey {
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
            // Begin scope to emsure ThreadRng is drop at compile time so that Oracle derive Send AutoTrait
            let mut rng = thread_rng();
            for _ in 0..digits {
                let mut sk_nonce = [0u8; 32];
                rng.fill_bytes(&mut sk_nonce);
                let oracle_r_kp =
                    secp256k1_zkp::KeyPair::from_seckey_slice(&self.app_state.secp, &sk_nonce)
                        .unwrap();
                let nonce = XOnlyPublicKey::from_keypair(&oracle_r_kp).0;
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
                            n.into(),
                            (*s).into(),
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
    use std::{path::Path, str::FromStr};

    use dlc_messages::oracle_msgs::{
        DigitDecompositionEventDescriptor, EventDescriptor, OracleAnnouncement, OracleAttestation,
        OracleEvent, Writeable,
    };
    use secp256k1_zkp::{rand, schnorr::Signature, KeyPair, Message, Secp256k1, XOnlyPublicKey};
    use sqlx_mock::TestPostgres;
    use time::{macros::datetime, Duration, OffsetDateTime};

    use crate::{
        common::{AssetPair, AssetPairInfo},
        pricefeeds::{
            ImplementedPriceFeed::{self, Lnm},
            PriceFeedError,
        },
    };

    use super::{
        crypto::test::check_signature_with_nonce, postgres::DBconnection, Oracle, OracleError,
    };

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
        let (secret_key, _) = secp.generate_keypair(&mut rand::thread_rng());
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
        let oracle_announcement = oracle.create_announcement(date.clone()).await.unwrap();
        let now = OffsetDateTime::now_utc();
        let oracle_attestation = match oracle
            .try_attest_event(oracle_announcement.oracle_event.event_id)
            .await
        {
            Ok(attestation) => attestation,
            Err(OracleError::PriceFeedError(error)) => {
                let PriceFeedError::PriceNotAvailableError(_, asked_date) = error else {panic!("Pricefeeder {:?} did not respond for this date {}", oracle.asset_pair_info.pricefeed, date.clone())};
                // Pricefeeder can only respond that price is not available if our query was asking in the future
                if asked_date < now {
                    panic!("Pricefeeder {:?} say price is not available for {}, which is not in the future (now it is: {}). Maybe only recent index are available.", oracle.asset_pair_info.pricefeed, asked_date, now)
                };
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
        // THIS TEST DOES NOT PASS BECAUSE SUREDBITS AND 10101 DO NOT FOLLOW THE SAME SPECIFICATION WE FOLLOW 10101 SPEC FROM RUSTDLC
        //
        // // https://oracle.suredbits.com/announcement/3ef91d749960d85f1190e86bd89d7d65a6303ca9bd4f16111c569d46d81f8f04
        // let attestation_test_vec = OracleAttestation {
        //     oracle_public_key: XOnlyPublicKey::from_str("04ba9838623f02c940d20d7b185d410178cff7990c7fcf19186c7f58c7c4b8de").unwrap(),
        //     signatures: [
        //         "02ef69281933974a279c4961adc96a442847739bd4af2d137fcac924438a223e0dd7103814689c706241f789a1f48a6670d24045b874e6956b0e673cc0160ecd",
        //         "1fdc6fcb01764771c4f6edd52aebb0cfc069953c83dddf9e8e03ba316e2799e0f00fd65a3f6358703e8dbf65598b105b895511fff6f64a10338df74feafbdf85",
        //         "3dbf7826a18233f7b214a1695a0453aaa36665a907d72544ccc3f73686e0959895e41c169d845f1c426b9a3453ca685c74734ed810f82ed3dc40744889f84683",
        //         "3e62346e411fbb6686eeb2e35f9cea155105aa40fef6c06680b672fda38a065bda4872e365b8caf15970fe98583c8d8009c9235ffdbcf8c53a930bde0178f8d8",
        //         "6a0839f21b194b785de639d22eb31441511e687e2a8c1d5d6ba00a3757a783b810a2af9ea505155fc516e9a6b64b62cbab3c19f0f8fad0dccce3fc23d7b716c6",
        //         "7d5a3e5a160b0d71349fb6baef76cffbfff4dcf512564edbb7b6c71cd334fd2882935abfd71e4d7754867addd8ef2a6d65ae2cf656c657bc3c8a04248c51ee48",
        //         "802d35ad5fe58a16852dbb9ca461d247415ceb340e30076b0c4e5b67ad87dd3917adcd183bf70495701017cd5261056ced452509235c179a5fff15f51b743f5b",
        //         "837731cc31462753c6f6431248e7c50223fa40905ae009711efe676007399ef35ed67e497289844059006763477db88a29ad440050cfe3ca2ee6be51b4494bd3",
        //         "963b3c8450b880e517861a3511e54dea2539206450327cfd03fd07a456e21d6a40478ea41699b491a2f9ebfd96a110ff1647ccc4708cdddd9922f1db5ec17548",
        //         "96597ceffd09c8465ec8dc2175a333b9b8ea9e7a0d560f171ca72883392683bf237433f303de77a19f6918a95419da6d231b2006e0d4b5dea7aee3e7aae70b87",
        //         "9a9a35ea727549c1ee00e60059fc07c6bae2fa185d75d9d4aa9339c14d96d7ef5cb78c1381958fb8293ca725077f9104a8342506bd34f4f3d8161a439338b3a7",
        //         "a6c76aa37f009e0be9a327fd2ca8ed211e10a7f88bdaf61fee01cff2cb47e8c05bb54a301e98d0d8b3b05471abf211f8280fb3b5b31bd2dfdaa1792fc70c9727",
        //         "c5b3474e1f2cef7cd95d083f3552848f23948255f569ca8c4077c3dc2852b18a552c9f6f825ed015dcca74b958ffc63eb0bcbe0cee8f42c7de17af2221424ae5",
        //         "d257ede428866f9dd4aa27e942afc1941e50cdf90728757de4c18a17816a4850ddf9ab9ad0a4acda2b8a7942b12d28184f18cab50c9fc2f2ad58c278dbef38d0",
        //         "d59f8b04317a3804668126293e985583457a47fcf27658217fe9018531a87c343f89e0fc722dc879248cfa9140f5820292a194240c4f7a768c42dfed7279af55",
        //         "dc511785d266148d215c096df7a0e71a6e7f54be2ef5ac2da435f7ddf9c27f2aad5fd6455c950d169dbbfe271c1b8cc39c7bc698ec58d95e60309eab6c6f10bf",
        //         "e16fe2dadb562db45ca3deaafc6917487a50c558cc1d8ca608a87163d6356ff565559301b79d753f226d806d87ad27df7711c99cafb39989ed28a3fe7becbcd1",
        //         "f31a6e3680e6142487754d1ba6790949e6f2d9ac1644036cdfae5edac773ba7e60d053288c14e93f280f8f0f21dd625950ac64ef1faa74997191368d9b742f85"
        //         ].into_iter().map(|s| Signature::from_str(s).unwrap()).collect(),
        //     outcomes: [
        //         "0",
        //         "0",
        //         "0",
        //         "1",
        //         "1",
        //         "0",
        //         "1",
        //         "1",
        //         "1",
        //         "0",
        //         "0",
        //         "1",
        //         "1",
        //         "1",
        //         "1",
        //         "1",
        //         "1",
        //         "1"
        //         ].into_iter().map(|d| d.to_string()).collect(),
        //     };

        // check_test_vec(attestation_test_vec);
    }
}
