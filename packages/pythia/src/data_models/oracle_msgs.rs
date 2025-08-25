use hex::ToHex;
use secp256k1_zkp::{schnorr::Signature, XOnlyPublicKey};
use serde::{ser::SerializeSeq, Deserialize, Serialize, Serializer};

use crate::data_models::{asset_pair::Unit, event_ids::EventId, ArrayString, Outcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitDecompositionEventDesc {
    pub base: u16,
    pub is_signed: bool,
    pub unit: Unit,
    pub precision: i32,
    pub nb_digits: u16,
}

impl Default for DigitDecompositionEventDesc {
    fn default() -> Self {
        Self {
            base: 2,
            is_signed: false,
            unit: "usd/btc".parse().expect("usd/btc len is 7"),
            precision: 0,
            nb_digits: 20,
        }
    }
}

type NonceBytes = [u8; 32];
struct NonceAsHex(ArrayString<64>);

impl FromIterator<char> for NonceAsHex {
    fn from_iter<T: IntoIterator<Item = char>>(iter: T) -> Self {
        let mut result = ArrayString::<64>::new();
        for c in iter {
            result.push(c);
        }
        Self(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// We store nonce as byte array so that we can cast them as
    /// one big slice when hashing them to sign the event.
    /// However we want to serialize them as hex strings in the API
    /// so we provide a custom serialisation function.
    #[serde(serialize_with = "serialize_nonces")]
    pub oracle_nonces: Vec<NonceBytes>,
    #[serde(rename = "eventMaturityEpoch")]
    pub maturity: u32,
    pub event_id: EventId,
    #[serde(serialize_with = "serialize_event_desc")]
    pub event_descriptor: DigitDecompositionEventDesc,
}

fn serialize_event_desc<S: Serializer>(
    event_desc: &DigitDecompositionEventDesc,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_newtype_variant("", 0, "digitDecompositionEvent", event_desc)
}

fn serialize_nonces<S>(bytes: &[NonceBytes], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq_serializer = serializer.serialize_seq(Some(bytes.len()))?;
    bytes
        .iter()
        .try_for_each(|b| seq_serializer.serialize_element(&b.encode_hex::<NonceAsHex>().0))?;
    seq_serializer.end()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Announcement {
    pub oracle_event: Event,
    pub announcement_signature: Signature,
    pub oracle_public_key: XOnlyPublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attestation {
    pub event_id: EventId,
    pub oracle_public_key: XOnlyPublicKey,
    pub signatures: Vec<Signature>,
    pub outcomes: Vec<Outcome>,
}

/// module to replicate the event serialisation from rust-dlc:
/// <https://github.com/p2pderivatives/rust-dlc/blob/v0.7.1/dlc-messages/src/oracle_msgs.rs#L246>
mod event_serialisation {
    use std::io::{sink, Write};

    /// Replicates the BigSize struct from rust-lightning serialisation:
    /// <https://github.com/rust-bitcoin/rust-lightning/blob/v0.0.101/lightning/src/util/ser.rs#L302>
    struct BigSize(u64);

    impl BigSize {
        fn write_to<W: Write>(self, writer: &mut W) -> Result<usize, std::io::Error> {
            match self.0 {
                0..=0xFC => {
                    writer.write_all(std::slice::from_ref(&(self.0 as u8)))?;
                    Ok(1)
                }
                0xFD..=0xFFFF => {
                    writer.write_all(&[0xFDu8])?;
                    writer.write_all(&(self.0 as u16).to_be_bytes())?;
                    Ok(3)
                }
                0x10000..=0xFFFFFFFF => {
                    writer.write_all(&[0xFEu8])?;
                    writer.write_all(&(self.0 as u32).to_be_bytes())?;
                    Ok(5)
                }
                _ => {
                    writer.write_all(&[0xFFu8])?;
                    writer.write_all(&self.0.to_be_bytes())?;
                    Ok(9)
                }
            }
        }
    }

    // Writes a string literal to the given writer
    fn write_string<W: Write>(input: &str, writer: &mut W) -> Result<usize, std::io::Error> {
        let len = BigSize(input.len() as u64);
        let count = len.write_to(writer)?;
        Ok(count + writer.write(input.as_bytes())?)
    }

    impl super::DigitDecompositionEventDesc {
        fn write_to<W: Write>(&self, writer: &mut W) -> Result<usize, std::io::Error> {
            let mut count = 0;
            count += writer.write(&self.base.to_be_bytes())?;
            count += writer.write(&[self.is_signed as u8])?;
            count += write_string(self.unit.as_ref(), writer)?;
            count += writer.write(&self.precision.to_be_bytes())?;
            Ok(count + writer.write(&self.nb_digits.to_be_bytes())?)
        }
    }

    impl super::Event {
        pub fn write_to(&self, writer: &mut impl Write) -> Result<(), std::io::Error> {
            let nb_nonces = self.oracle_nonces.len() as u16;
            writer.write_all(&nb_nonces.to_be_bytes())?;

            // Because we store nonce as bytes array we can force the compiler to
            // flatten the array by casting it to a single slice to optimize hashing.
            writer.write_all(self.oracle_nonces.as_flattened())?;

            writer.write_all(&self.maturity.to_be_bytes())?;

            const DIGIT_DECOMPOSITION_EVENT_DESC_BIG_SIZE: u64 = 55306;
            BigSize(DIGIT_DECOMPOSITION_EVENT_DESC_BIG_SIZE).write_to(writer)?;

            let count = self
                .event_descriptor
                .write_to(&mut sink())
                .expect("write to empty cannot fail");
            BigSize(count as u64).write_to(writer)?;

            self.event_descriptor.write_to(writer)?;

            write_string(&self.event_id, writer)?;

            Ok(())
        }
    }

    #[cfg(test)]
    mod test_ser {
        use super::super::*;

        use super::super::Event as OurOracleEvent;

        use chrono::{Duration, Utc};

        use dlc_messages::oracle_msgs::OracleEvent as RustDlcOracleEvent;
        use lightning::util::ser::Writeable;
        use secp256k1_zkp::rand::thread_rng;

        use crate::{data_models::asset_pair::AssetPair, SECP};

        #[test]
        fn test_event_serialisation_matches_rust_dlc() {
            let now = Utc::now();

            let oracle_nonces = {
                let mut rng = thread_rng();
                std::iter::repeat_with(|| {
                    SECP.generate_keypair(&mut rng)
                        .1
                        .x_only_public_key()
                        .0
                        .serialize()
                        .into()
                })
                .take(5)
                .collect()
            };

            let our_event = OurOracleEvent {
                oracle_nonces,
                maturity: (now + Duration::hours(1)).timestamp() as u32,
                event_id: EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, now),
                event_descriptor: DigitDecompositionEventDesc::default(),
            };

            // Encode our event with our implementation
            let mut our_ser_result = Vec::with_capacity(250);
            our_event
                .write_to(&mut our_ser_result)
                .expect("write to vec cannot fail");

            // Encode the event with rust-dlc implementation
            let rust_dlc_encoded = RustDlcOracleEvent::from(our_event).encode();

            // Both encodings must match exactly:
            assert_eq!(rust_dlc_encoded[..], our_ser_result[..]);
        }
    }
}

#[cfg(test)]
mod conversions_to_rust_dlc_types {
    use dlc_messages::oracle_msgs::{
        DigitDecompositionEventDescriptor, EventDescriptor, OracleAnnouncement, OracleAttestation,
        OracleEvent,
    };
    use secp256k1_zkp::XOnlyPublicKey;

    use super::{Announcement, Attestation, DigitDecompositionEventDesc, Event};

    impl From<Event> for OracleEvent {
        fn from(
            Event {
                oracle_nonces,
                maturity,
                event_id,
                event_descriptor:
                    DigitDecompositionEventDesc {
                        base,
                        is_signed,
                        unit,
                        precision,
                        nb_digits,
                    },
            }: Event,
        ) -> Self {
            OracleEvent {
                oracle_nonces: oracle_nonces
                    .iter()
                    .map(|nonce| XOnlyPublicKey::from_slice(nonce.as_ref()).unwrap())
                    .collect(),
                event_id: event_id.to_string(),
                event_maturity_epoch: maturity,
                event_descriptor: EventDescriptor::DigitDecompositionEvent(
                    DigitDecompositionEventDescriptor {
                        base,
                        is_signed,
                        unit: unit.to_string(),
                        precision,
                        nb_digits,
                    },
                ),
            }
        }
    }

    impl From<Announcement> for OracleAnnouncement {
        fn from(
            Announcement {
                announcement_signature,
                oracle_public_key,
                oracle_event,
            }: Announcement,
        ) -> Self {
            OracleAnnouncement {
                announcement_signature,
                oracle_public_key,
                oracle_event: oracle_event.into(),
            }
        }
    }

    impl From<Attestation> for OracleAttestation {
        fn from(attestation: Attestation) -> Self {
            OracleAttestation {
                event_id: attestation.event_id.to_string(),
                oracle_public_key: attestation.oracle_public_key,
                signatures: attestation.signatures,
                outcomes: attestation
                    .outcomes
                    .iter()
                    .map(|o| o.0.to_string())
                    .collect(),
            }
        }
    }
}

#[cfg(test)]
mod test_serde_compatibility {
    use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};

    use super::*;

    #[test]
    fn test_announcement_serde() {
        let event = Event {
            oracle_nonces: vec![[1u8; 32], [2u8; 32]],
            maturity: 1234567,
            event_id: "btc_usdTenLetters".parse().unwrap(),
            event_descriptor: DigitDecompositionEventDesc {
                base: 2,
                is_signed: false,
                unit: "usd/btc".parse().unwrap(),
                precision: 8,
                nb_digits: 30,
            },
        };

        let announcement = Announcement {
            oracle_event: event,
            announcement_signature: Signature::from_slice(&[3u8; 64]).unwrap(),
            oracle_public_key: XOnlyPublicKey::from_slice(&[1u8; 32]).unwrap(),
        };

        let json = serde_json::to_string(&announcement).unwrap();
        let deserialized: OracleAnnouncement = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.oracle_public_key,
            announcement.oracle_public_key
        );
        assert_eq!(
            deserialized.announcement_signature,
            announcement.announcement_signature
        );
        assert_eq!(
            deserialized.oracle_event.event_id,
            announcement.oracle_event.event_id.to_string()
        );
    }

    #[test]
    fn test_attestation_serde() {
        let attestation = Attestation {
            event_id: "btc_usdTenLetters".parse().unwrap(),
            oracle_public_key: XOnlyPublicKey::from_slice(&[1u8; 32]).unwrap(),
            signatures: vec![Signature::from_slice(&[3u8; 64]).unwrap()],
            outcomes: vec![Outcome::try_from('1').unwrap()],
        };

        let json = serde_json::to_string(&attestation).unwrap();
        let deserialized: OracleAttestation = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.oracle_public_key,
            attestation.oracle_public_key
        );
        assert_eq!(deserialized.signatures, attestation.signatures);
        assert_eq!(deserialized.event_id, attestation.event_id.to_string());
        assert_eq!(
            deserialized.outcomes,
            attestation
                .outcomes
                .iter()
                .map(|o| o.0.to_string())
                .collect::<Vec<_>>()
        );
    }
}
