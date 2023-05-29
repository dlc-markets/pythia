use crate::{AssetPairInfo, OracleConfig};
use bit_vec::BitVec;
use displaydoc::Display;
use dlc_messages::oracle_msgs::{
    DigitDecompositionEventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent,
};
use secp256k1_zkp::{KeyPair, XOnlyPublicKey as SchnorrPublicKey};
use std::time::SystemTime;
use std::{str::FromStr, sync::Arc};
use time::OffsetDateTime;

use tokio_postgres::types::ToSql;

use secp256k1_zkp::{schnorr::Signature, Scalar, XOnlyPublicKey};

mod error;
pub use error::OracleError;
pub use error::Result;
pub mod postgres;

#[derive(Clone)]
// outstanding_sk_nonces?, announcement, attetstation?, outcome?
pub struct DbValue(
    pub Option<Vec<[u8; 32]>>,
    pub OracleAnnouncement,
    pub Option<OracleAttestation>,
    pub Option<u32>,
);
#[derive(Clone)]
pub enum ScalarPart {
    AnnouncementSkNonce(Vec<[u8; 32]>),
    Attestation(OracleAttestation),
}

pub struct OracleSignature(Signature);
pub struct NoncePoint(XOnlyPublicKey);
#[derive(Display)]
pub struct SigningScalar(Scalar);

impl From<OracleSignature> for (NoncePoint, SigningScalar) {
    fn from(sig: OracleSignature) -> Self {
        let (x_nonce_bytes, scalar_bytes) = sig.0.as_ref().split_at(32);
        let scalar_array = scalar_bytes
            .try_into()
            .expect("Schnorr signature is 64 bytes long");
        (
            NoncePoint(
                XOnlyPublicKey::from_slice(x_nonce_bytes).expect("signature split correctly"),
            ),
            SigningScalar(
                Scalar::from_be_bytes(scalar_array)
                    .expect("signature scalar is always less then curve order"),
            ),
        )
    }
}

impl From<(NoncePoint, SigningScalar)> for OracleSignature {
    fn from(value: (NoncePoint, SigningScalar)) -> Self {
        OracleSignature(
            Signature::from_str(
                (value.0 .0.to_string() + value.0 .0.to_string().as_str()).as_str(),
            )
            .expect("Nonce and scalar are 64 bytes long"),
        )
    }
}

#[derive(Clone)]
pub struct PostgresResponse {
    pub announcement: OracleAnnouncement,
    pub scalar_part: ScalarPart,
}
#[derive(Clone)]
pub struct Oracle {
    pub oracle_config: OracleConfig,
    asset_pair_info: AssetPairInfo,
    // pub event_database: Db,
    pub postgres_client: Arc<DatabaseConnection>,
    keypair: KeyPair,
}

impl Oracle {
    pub fn new(
        oracle_config: OracleConfig,
        asset_pair_info: AssetPairInfo,
        postgres_client: Arc<DatabaseConnection>,
        keypair: KeyPair,
    ) -> Result<Oracle> {
        if !oracle_config.announcement_offset.is_positive() {
            return Err(OracleError::InvalidAnnouncementTimeError(
                oracle_config.announcement_offset,
            ));
        }

        // setup event database
        // let path = format!("events/{}", asset_pair_info.asset_pair);
        // info!("creating sled at {}", path);
        // let event_database = sled::open(path)?;

        Ok(Oracle {
            oracle_config,
            asset_pair_info,
            // event_database,
            postgres_client,
            keypair,
        })
    }
    pub fn get_public_key(self: &Oracle) -> SchnorrPublicKey {
        self.keypair.public_key().into()
    }

    pub async fn insert_announcement(
        &self,
        announcement: OracleAnnouncement,
        outstanding_sk_nonces: Vec<[u8; 32]>,
    ) -> Result<()> {
        let EventDescriptor::DigitDecompositionEvent(digits) = announcement.oracle_event.event_descriptor else {
          return Err(OracleError::IndexingError())
        };
        let params: &[&(dyn ToSql + Sync)] = &[
            &announcement.oracle_event.event_id,
            &(digits.nb_digits as u32),
            &digits.precision,
            &SystemTime::from(
                OffsetDateTime::from_unix_timestamp(
                    announcement
                        .oracle_event
                        .event_maturity_epoch
                        .try_into()
                        .unwrap(),
                )
                .expect("Stored timestamp is valid"),
            ),
            &BitVec::from_bytes(announcement.announcement_signature.as_ref()),
        ];
        // info!("Insert: {:?}", params);
        self.postgres_client
            .client
            .query(&self.postgres_client.insert_announcement_statement, params)
            .await?;

        for (i, sk_nounce) in outstanding_sk_nonces.iter().enumerate() {
            let params: &[&(dyn ToSql + Sync)] = &[
                &announcement.oracle_event.event_id,
                &(i as u32),
                &BitVec::from_bytes(
                    announcement.oracle_event.oracle_nonces[i]
                        .serialize()
                        .as_slice(),
                ),
                &BitVec::from_bytes(sk_nounce),
            ];
            self.postgres_client
                .client
                .query(&self.postgres_client.insert_attestation_statement, params)
                .await?;
        }
        Ok(())
    }

    pub async fn attestion_update(
        &self,
        event_id: &str,
        attestation: OracleAttestation,
        outcome: u32,
    ) -> Result<()> {
        let params: &[&(dyn ToSql + Sync)] = &[&outcome, &event_id];
        // info!("Update: {:?}", params);
        self.postgres_client
            .client
            .query(&self.postgres_client.update_announcement_statement, params)
            .await?;

        for (index, (bit, sig)) in attestation
            .outcomes
            .iter()
            .zip(attestation.signatures.into_iter().map(|schnorr_sig| {
                <OracleSignature as Into<(NoncePoint, SigningScalar)>>::into(OracleSignature(
                    schnorr_sig,
                ))
                .1
            }))
            .enumerate()
        {
            let params: &[&(dyn ToSql + Sync)] = &[
                &BitVec::from_elem(1, i32::from_str(bit.as_str()).unwrap() != 0),
                &BitVec::from_bytes(&sig.0.to_be_bytes()),
                &event_id,
                &(index as u32),
            ];
            self.postgres_client
                .client
                .query(&self.postgres_client.update_attestation_statement, params)
                .await?;
        }
        Ok(())
    }

    pub async fn get_oracle_event(&self, event_id: String) -> Result<Option<PostgresResponse>> {
        let params: [&(dyn ToSql + Sync); 1] = [&event_id];
        let event_raws = self
            .postgres_client
            .client
            .query(
                &self.postgres_client.get_oracle_event_info_statement,
                &params,
            )
            .await?;
        let [event_raw] = event_raws.as_slice()  else { return Ok(None)};
        let digits_rows = self
            .postgres_client
            .client
            .query(&self.postgres_client.get_oracle_event_statement, &params)
            .await?;
        let (nb_digits, precision, event_maturity_epoch, announcement_signature, outcome_option): (
            u16,
            i32,
            u32,
            Signature,
            Option<i32>,
        ) = (
            event_raw
                .get::<usize, i32>(1)
                .try_into()
                .expect("to never be that high"),
            event_raw.get(2),
            OffsetDateTime::from(event_raw.get::<usize, SystemTime>(3))
                .unix_timestamp()
                .try_into()
                .expect("timestamp low"),
            Signature::from_slice(event_raw.get::<usize, BitVec>(4).to_bytes().as_slice()).unwrap(),
            event_raw.get(5),
        );

        match outcome_option {
            None => {
                let mut aggregated_rows: Vec<(i32, (XOnlyPublicKey, [u8; 32]))> = digits_rows
                    .iter()
                    .map(|x| {
                        (
                            x.get::<usize, i32>(1),
                            (
                                XOnlyPublicKey::from_slice(&x.get::<usize, BitVec>(2).to_bytes())
                                    .unwrap(),
                                x.get::<usize, BitVec>(3).to_bytes().try_into().unwrap(),
                            ),
                        )
                    })
                    .collect();
                aggregated_rows.sort_unstable_by_key(|&d| d.0);
                type AnnouncementRow = (Vec<i32>, (Vec<XOnlyPublicKey>, Vec<[u8; 32]>));
                let (_, (nonce_public, nonce_secret)): AnnouncementRow =
                    aggregated_rows.into_iter().unzip();
                Ok(Some(PostgresResponse {
                    announcement: OracleAnnouncement {
                        announcement_signature,
                        oracle_public_key: self.keypair.x_only_public_key().0,
                        oracle_event: OracleEvent {
                            oracle_nonces: nonce_public,
                            event_maturity_epoch,
                            event_descriptor: EventDescriptor::DigitDecompositionEvent(
                                DigitDecompositionEventDescriptor {
                                    base: 2,
                                    is_signed: false,
                                    unit: "usd/btc".to_owned(),
                                    precision,
                                    nb_digits,
                                },
                            ),
                            event_id,
                        },
                    },
                    scalar_part: ScalarPart::AnnouncementSkNonce(nonce_secret),
                }))
            }
            Some(_) => {
                type AggregatedRows = (i32, (XOnlyPublicKey, (String, Signature)));
                let mut aggregated_rows: Vec<AggregatedRows> = digits_rows
                    .iter()
                    .map(|x| {
                        let nonce_x =
                            XOnlyPublicKey::from_slice(&x.get::<usize, BitVec>(2).to_bytes())
                                .unwrap();
                        (
                            x.get::<usize, i32>(1),
                            (
                                nonce_x,
                                (
                                    (x.get::<usize, bit_vec::BitVec>(4).pop().unwrap() as i8)
                                        .to_string(),
                                    <(NoncePoint, SigningScalar) as Into<OracleSignature>>::into((
                                        NoncePoint(nonce_x),
                                        SigningScalar(
                                            Scalar::from_be_bytes(
                                                x.get::<usize, bit_vec::BitVec>(5)
                                                    .to_bytes()
                                                    .try_into()
                                                    .unwrap(),
                                            )
                                            .unwrap(),
                                        ),
                                    ))
                                    .0,
                                ),
                            ),
                        )
                    })
                    .collect();
                aggregated_rows.sort_unstable_by_key(|d| d.0);
                type AttestationRaws = (
                    Vec<i32>,
                    (Vec<XOnlyPublicKey>, (Vec<String>, Vec<Signature>)),
                );
                let (_, (nonce_public, (bits, sigs))): AttestationRaws =
                    aggregated_rows.into_iter().unzip();
                Ok(Some(PostgresResponse {
                    announcement: OracleAnnouncement {
                        announcement_signature,
                        oracle_public_key: self.keypair.x_only_public_key().0,
                        oracle_event: OracleEvent {
                            oracle_nonces: nonce_public,
                            event_maturity_epoch,
                            event_descriptor: EventDescriptor::DigitDecompositionEvent(
                                DigitDecompositionEventDescriptor {
                                    base: 2,
                                    is_signed: false,
                                    unit: "usd/btc".to_owned(),
                                    precision,
                                    nb_digits,
                                },
                            ),
                            event_id,
                        },
                    },
                    scalar_part: ScalarPart::Attestation(OracleAttestation {
                        oracle_public_key: self.keypair.x_only_public_key().0,
                        signatures: sigs,
                        outcomes: bits,
                    }),
                }))
            }
        }
    }
}

pub mod oracle_scheduler;
pub use dlc_messages::oracle_msgs::EventDescriptor;

use self::postgres::DatabaseConnection;

pub mod pricefeeds;
