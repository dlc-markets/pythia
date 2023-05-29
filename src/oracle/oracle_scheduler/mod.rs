use super::{
    pricefeeds::{PriceFeed, Result as PriceFeedResult},
    Oracle, PostgresResponse,
};
use crate::{oracle::ScalarPart, AssetPairInfo};
use chrono::Utc;
use clokwerk::{AsyncScheduler, Interval};
use futures::{stream, StreamExt};
use log::info;
use queues::{queue, IsQueue, Queue};
use secp256k1_zkp::{
    hashes::*,
    rand::{self, RngCore},
    All, KeyPair, Message, Secp256k1, XOnlyPublicKey as SchnorrPublicKey,
};
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::{
    sync::{mpsc, Mutex},
    time::sleep,
};

mod error;
pub use error::OracleSchedulerError;
pub use error::Result;

use dlc::secp_utils::schnorrsig_sign_with_nonce;
use dlc_messages::oracle_msgs::{
    EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent, Writeable,
};
extern crate hex;

const SCHEDULER_SLEEP_TIME: std::time::Duration = std::time::Duration::from_millis(100);

struct OracleScheduler {
    oracle: Oracle,
    secp: Secp256k1<All>,
    pricefeeds: Vec<Box<dyn PriceFeed + Send + Sync>>,
    db_values: Queue<PostgresResponse>,
    next_announcement: OffsetDateTime,
    next_attestation: OffsetDateTime,
}

impl OracleScheduler {
    async fn create_scheduler_event(&mut self) -> Result<()> {
        let announcement_offset = self.oracle.oracle_config.announcement_offset;
        create_event(
            &mut self.oracle,
            &self.secp,
            &mut self.db_values,
            self.next_announcement + announcement_offset,
        )
        .await?;
        self.next_announcement += self.oracle.oracle_config.frequency;
        Ok(())
    }

    async fn attest(&mut self) -> Result<()> {
        info!("retrieving pricefeeds for attestation");
        let prices = stream::iter(self.pricefeeds.iter())
            .then(|pricefeed| async {
                pricefeed
                    .retrieve_price(
                        self.oracle.asset_pair_info.asset_pair,
                        self.next_attestation,
                    )
                    .await
            })
            .collect::<Vec<PriceFeedResult<_>>>()
            .await
            .into_iter()
            .collect::<PriceFeedResult<Vec<_>>>()?;
        let avg_price = prices.iter().sum::<f64>() / prices.len() as f64;
        let avg_price = avg_price.round() as u32;
        info!(
            "average price of {} is {}",
            self.oracle.asset_pair_info.asset_pair, avg_price
        );
        let EventDescriptor::DigitDecompositionEvent(event) =
            &self.oracle.asset_pair_info.event_descriptor else {panic!("Wrong event in db")};
        let avg_price_binary = format!("{:0width$b}", avg_price, width = event.nb_digits as usize);
        let outcomes = avg_price_binary
            .chars()
            .map(|char| char.to_string())
            .collect::<Vec<_>>();
        let PostgresResponse { announcement,scalar_part: ScalarPart::AnnouncementSkNonce(outstanding_sk_nonces) } = self
            .db_values
            .remove()
            .expect("db_values should never be empty") else { return Err(OracleSchedulerError::IndexingError()) };

        let attestation = build_attestation(
            outstanding_sk_nonces,
            &self.oracle.keypair,
            &self.secp,
            outcomes,
        );

        info!(
            "attesting with maturation {} and attestation {:#?}",
            self.next_attestation, attestation
        );
        let event_id = announcement.oracle_event.event_id;
        self.oracle
            .attestion_update(&event_id, attestation, avg_price)
            .await
            .unwrap();
        self.next_attestation += self.oracle.oracle_config.frequency;
        Ok(())
    }
}

pub fn init(
    oracle: Oracle,
    secp: Secp256k1<All>,
    pricefeeds: Vec<Box<dyn PriceFeed + Send + Sync>>,
) -> Result<()> {
    // start event creation task
    info!("creating oracle events and schedules");
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel();
        if let Err(err) = create_events(oracle, secp, pricefeeds, tx).await {
            panic!("oracle scheduler create_events error: {}", err);
        }
        while let Some(err) = rx.recv().await {
            panic!("oracle scheduler error: {}", err);
        }
        // never be reached
        unreachable!()
    });
    Ok(())
}

async fn create_events(
    mut oracle: Oracle,
    secp: Secp256k1<All>,
    pricefeeds: Vec<Box<dyn PriceFeed + Send + Sync>>,
    error_transmitter: mpsc::UnboundedSender<OracleSchedulerError>,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let mut next_attestation = now
        .replace_second(0)
        .expect("0 seconde is valid")
        .replace_millisecond(0)
        .expect("Millisecond can be 0");
    if next_attestation <= now {
        next_attestation += oracle.oracle_config.frequency;
    }
    let mut next_announcement = next_attestation - oracle.oracle_config.announcement_offset;
    let mut db_values = queue![];
    // create all events that should have already been made
    info!("creating events that should have already been made");
    while next_announcement <= now {
        let next_attestation = next_announcement + oracle.oracle_config.announcement_offset;
        match oracle
            .get_oracle_event("btcusd".to_string() + &next_attestation.unix_timestamp().to_string())
            .await
            .unwrap()
        {
            None => create_event(&mut oracle, &secp, &mut db_values, next_attestation).await?,
            Some(val) => {
                info!(
                    "existing oracle event found in db with maturation {}, skipping creation",
                    next_attestation
                );
                db_values.add(val).unwrap();
            }
        };
        next_announcement += oracle.oracle_config.frequency;
    }
    let oracle_scheduler = Arc::new(Mutex::new(OracleScheduler {
        oracle: oracle.clone(),
        secp,
        pricefeeds,
        db_values,
        next_announcement,
        next_attestation,
    }));
    info!(
        "created new oracle scheduler with\n\tannouncements at {}\n\tattestations at {}\n\tfrequency of {}\n\tnext announcement at {}\n\tnext attestation at {}",
        oracle.oracle_config.attestation_time - oracle.oracle_config.announcement_offset,
        oracle.oracle_config.attestation_time,
        oracle.oracle_config.frequency,
        next_announcement,
        next_attestation
    );

    let mut scheduler = AsyncScheduler::with_tz(Utc);
    // schedule announcements
    let error_transmitter_clone = error_transmitter.clone();
    let oracle_scheduler_clone = oracle_scheduler.clone();
    let interval = Interval::Seconds(
        oracle
            .oracle_config
            .frequency
            .whole_seconds()
            .try_into()
            .unwrap(),
    );
    info!("starting announcement scheduler");
    scheduler.every(interval).run(move || {
        let oracle_scheduler_clone = oracle_scheduler_clone.clone();
        let error_transmitter_clone = error_transmitter_clone.clone();
        async move {
            if let Err(err) = oracle_scheduler_clone
                .lock()
                .await
                .create_scheduler_event()
                .await
            {
                info!("error from announcement scheduler");
                error_transmitter_clone.send(err).unwrap();
            }
        }
    });
    // schedule attestations
    info!("starting attestation scheduler");
    scheduler.every(interval).run(move || {
        let oracle_scheduler_clone = oracle_scheduler.clone();
        let error_transmitter_clone = error_transmitter.clone();
        async move {
            if let Err(err) = oracle_scheduler_clone.lock().await.attest().await {
                info!("error from attestation scheduler");
                error_transmitter_clone.send(err).unwrap();
            }
        }
    });
    // busy checking scheduler
    info!("starting busy checking");
    tokio::spawn(async move {
        loop {
            scheduler.run_pending().await;
            sleep(SCHEDULER_SLEEP_TIME).await;
        }
    });
    Ok(())
}

async fn create_event(
    oracle: &mut Oracle,
    secp: &Secp256k1<All>,
    db_values: &mut Queue<PostgresResponse>,
    maturation: OffsetDateTime,
) -> Result<()> {
    let (announcement, outstanding_sk_nonces) =
        build_announcement(&oracle.asset_pair_info, &oracle.keypair, secp, maturation)?;

    let db_value = PostgresResponse {
        announcement: announcement.clone(),
        scalar_part: ScalarPart::AnnouncementSkNonce(outstanding_sk_nonces.clone()),
    };
    info!(
        "creating oracle event (announcement only) with maturation {} and announcement {:#?}",
        maturation, &announcement
    );
    oracle
        .insert_announcement(announcement, outstanding_sk_nonces)
        .await
        .unwrap();
    db_values.add(db_value).unwrap();
    Ok(())
}

pub fn build_announcement(
    asset_pair_info: &AssetPairInfo,
    keypair: &KeyPair,
    secp: &Secp256k1<All>,
    maturation: OffsetDateTime,
) -> Result<(OracleAnnouncement, Vec<[u8; 32]>)> {
    let mut rng = rand::thread_rng();
    let EventDescriptor::DigitDecompositionEvent(event) = &asset_pair_info.event_descriptor else {panic!("Error in db")};
    let digits = event.nb_digits;
    let mut sk_nonces = Vec::with_capacity(digits.into());
    let mut nonces = Vec::with_capacity(digits.into());
    for _ in 0..digits {
        let mut sk_nonce = [0u8; 32];
        rng.fill_bytes(&mut sk_nonce);
        let oracle_r_kp = secp256k1_zkp::KeyPair::from_seckey_slice(secp, &sk_nonce)?;
        let nonce = SchnorrPublicKey::from_keypair(&oracle_r_kp).0;
        sk_nonces.push(sk_nonce);
        nonces.push(nonce);
    }

    let oracle_event = OracleEvent {
        oracle_nonces: nonces,
        event_maturity_epoch: maturation.unix_timestamp() as u32,
        event_descriptor: asset_pair_info.event_descriptor.clone(),
        event_id: asset_pair_info.asset_pair.to_string().to_lowercase()
            + maturation.unix_timestamp().to_string().as_str(),
    };

    Ok((
        OracleAnnouncement {
            announcement_signature: secp.sign_schnorr(
                &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                    &oracle_event.encode(),
                ),
                keypair,
            ),
            oracle_public_key: keypair.public_key().into(),
            oracle_event,
        },
        sk_nonces,
    ))
}

pub fn build_attestation(
    outstanding_sk_nonces: Vec<[u8; 32]>,
    keypair: &KeyPair,
    secp: &Secp256k1<All>,
    outcomes: Vec<String>,
) -> OracleAttestation {
    let signatures = outcomes
        .iter()
        .zip(outstanding_sk_nonces.iter())
        .map(|(outcome, outstanding_sk_nonce)| {
            schnorrsig_sign_with_nonce(
                secp,
                &Message::from_hashed_data::<sha256::Hash>(outcome.as_bytes()),
                keypair,
                outstanding_sk_nonce,
            )
        })
        .collect::<Vec<_>>();
    OracleAttestation {
        oracle_public_key: keypair.public_key().into(),
        signatures,
        outcomes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{oracle::EventDescriptor, AssetPair};
    use dlc::OracleInfo;
    use dlc_messages::oracle_msgs::DigitDecompositionEventDescriptor;
    use secp256k1_zkp::rand::{distributions::Alphanumeric, Rng};
    use secp256k1_zkp::scalar::Scalar;
    use secp256k1_zkp::schnorr::Signature as SchnorrSignature;

    fn setup() -> (KeyPair, Secp256k1<All>) {
        let secp = Secp256k1::new();
        let mut rng = rand::thread_rng();
        let (secret_key, _) = secp.generate_keypair(&mut rng);
        (KeyPair::from_secret_key(&secp, &secret_key), secp)
    }

    fn setup_v5() -> (
        secp256k1_zkp::SecretKey,
        secp256k1_zkp::PublicKey,
        secp256k1_zkp::Secp256k1<secp256k1_zkp::All>,
    ) {
        let secp = secp256k1_zkp::Secp256k1::new();
        let mut rng = rand::thread_rng();
        let (secret_key, public_key) = secp.generate_keypair(&mut rng);
        (secret_key, public_key, secp)
    }

    fn signatures_to_secret(signatures: &[SchnorrSignature]) -> secp256k1_zkp::SecretKey {
        let s_values: Vec<&[u8]> = signatures
            .iter()
            .map(|x| {
                let bytes = x.as_ref();
                &bytes[32..64]
            })
            .collect();
        let secret = secp256k1_zkp::SecretKey::from_slice(s_values[0]).unwrap();
        for s in s_values.iter().skip(1) {
            secret
                .add_tweak(
                    &Scalar::from_be_bytes((*s).try_into().expect("Length is good")).unwrap(),
                )
                .unwrap();
        }

        secret
    }

    #[test]
    fn announcement_signature_verifies() {
        let (keypair, secp) = setup();

        let announcement = build_announcement(
            &AssetPairInfo {
                asset_pair: AssetPair::BTCUSD,
                event_descriptor: EventDescriptor::DigitDecompositionEvent({
                    DigitDecompositionEventDescriptor {
                        base: 2,
                        is_signed: false,
                        unit: "BTCUSD".to_string(),
                        precision: 0,
                        nb_digits: 18,
                    }
                }),
            },
            &keypair,
            &secp,
            OffsetDateTime::now_utc(),
        )
        .unwrap()
        .0;

        // The spec say that we should tag the hash of oracle announcement with b"announcement/v0", this is actually not done in rust-dlc
        // let tag_hash = sha256::Hash::hash(b"announcement/v0");

        secp.verify_schnorr(
            &announcement.announcement_signature,
            &Message::from_hashed_data::<sha256::Hash>(
                &[announcement.oracle_event.encode()].concat(),
            ),
            &keypair.public_key().x_only_public_key().0,
        )
        .unwrap();
    }

    #[test]
    fn attestation_signature_verifies() {
        let (keypair, secp) = setup();

        let mut outstanding_sk_nonce = vec![[0u8; 32]];
        let mut rng = rand::thread_rng();
        rng.fill_bytes(&mut outstanding_sk_nonce[0]);
        let outcome = rng
            .sample_iter(&Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();
        let outcome = vec![outcome];
        let attestation = build_attestation(outstanding_sk_nonce, &keypair, &secp, outcome);
        secp.verify_schnorr(
            &attestation.signatures[0],
            &Message::from_hashed_data::<sha256::Hash>(attestation.outcomes[0].as_bytes()),
            &keypair.public_key().x_only_public_key().0,
        )
        .unwrap();
    }

    #[test]
    fn valid_adaptor_signature() {
        let (keypair, secp) = setup();

        let (announcement, outstanding_sk_nonces) = build_announcement(
            &AssetPairInfo {
                asset_pair: AssetPair::BTCUSD,
                event_descriptor: EventDescriptor::DigitDecompositionEvent({
                    DigitDecompositionEventDescriptor {
                        base: 2,
                        is_signed: false,
                        unit: "BTCUSD".to_string(),
                        precision: 0,
                        nb_digits: 18,
                    }
                }),
            },
            &keypair,
            &secp,
            OffsetDateTime::now_utc(),
        )
        .unwrap();

        let outcomes: Vec<String> = vec![
            "0", "0", "0", "1", "1", "1", "0", "1", "0", "0", "0", "1", "0", "1", "0", "0", "0",
            "1",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let attestation =
            build_attestation(outstanding_sk_nonces, &keypair, &secp, outcomes.clone());

        let (funding_secret_key, funding_public_key, secp) = setup_v5();

        let adaptor_point = dlc::get_adaptor_point_from_oracle_info(
            &secp,
            &[OracleInfo {
                public_key: secp256k1_zkp::XOnlyPublicKey::from_slice(
                    &keypair.public_key().x_only_public_key().0.serialize(),
                )
                .unwrap(),
                nonces: announcement
                    .oracle_event
                    .oracle_nonces
                    .iter()
                    .map(|nonce| {
                        secp256k1_zkp::XOnlyPublicKey::from_slice(&nonce.serialize()).unwrap()
                    })
                    .collect(),
            }],
            &[outcomes
                .iter()
                .map(|outcome| {
                    secp256k1_zkp::Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(
                        outcome.as_bytes(),
                    )
                })
                .collect::<Vec<_>>()],
        )
        .unwrap();

        let test_msg = secp256k1_zkp::Message::from_hashed_data::<
            secp256k1_zkp::hashes::sha256::Hash,
        >("test".as_bytes());
        let adaptor_sig = secp256k1_zkp::EcdsaAdaptorSignature::encrypt(
            &secp,
            &test_msg,
            &funding_secret_key,
            &adaptor_point,
        );

        adaptor_sig
            .verify(&secp, &test_msg, &funding_public_key, &adaptor_point)
            .unwrap();

        let adapted_sig = adaptor_sig
            .decrypt(&signatures_to_secret(&attestation.signatures))
            .unwrap();

        secp.verify_ecdsa(&test_msg, &adapted_sig, &funding_public_key)
            .unwrap();
    }
}
