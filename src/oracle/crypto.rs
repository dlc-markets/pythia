use std::str::FromStr;

use displaydoc::Display;

use dlc::secp_utils::schnorrsig_sign_with_nonce;
use secp256k1_zkp::{
    hashes::sha256, schnorr::Signature, All, KeyPair, Message, Scalar, Secp256k1, XOnlyPublicKey,
};

pub(super) struct OracleSignature(pub(super) Signature);
pub(super) struct NoncePoint(pub(super) XOnlyPublicKey);
#[derive(Display)]
pub(super) struct SigningScalar(pub(super) Scalar);

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

impl From<OracleSignature> for Signature {
    fn from(sig: OracleSignature) -> Self {
        sig.0
    }
}

pub fn to_digit_decomposition_vec(outcome: f64, digits: u16, precision: u16) -> Vec<String> {
    let outcome_rounded = (outcome * ((2_u64.pow(precision as u32)) as f64)).round() as u64;
    let outcome_binary = format!("{:0width$b}", outcome_rounded, width = digits as usize);
    outcome_binary
        .chars()
        .map(|char| char.to_string())
        .collect::<Vec<_>>()
}

pub(super) fn sign_outcome(
    secp: &Secp256k1<All>,
    keypair: &KeyPair,
    outcome: &String,
    outstanding_sk_nonce: &[u8; 32],
) -> (String, Signature) {
    (
        outcome.to_owned(),
        schnorrsig_sign_with_nonce(
            secp,
            &Message::from_hashed_data::<sha256::Hash>(outcome.as_bytes()),
            keypair,
            &outstanding_sk_nonce,
        ),
    )
}
