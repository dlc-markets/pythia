use std::{str::FromStr, sync::Arc};

use displaydoc::Display;

use secp256k1_zkp::{schnorr::Signature, Scalar, XOnlyPublicKey};

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

pub fn to_digit_decomposition_vec(outcome: u32, digits: u16) -> Vec<String> {
    let outcome_binary = format!("{:0width$b}", outcome, width = digits as usize);
    outcome_binary
        .chars()
        .map(|char| char.to_string())
        .collect::<Vec<_>>()
}
