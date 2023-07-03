use std::str::FromStr;

use displaydoc::Display;

use derive_more::From;
use dlc::secp_utils::schnorrsig_sign_with_nonce;
use secp256k1_zkp::{
    hashes::sha256, schnorr::Signature, All, KeyPair, Message, Scalar, Secp256k1, XOnlyPublicKey,
};

/// We use custom types to implement signature splitting into nonce and scalar.
/// Custum public nonce type
#[derive(From)]
pub(super) struct NoncePoint(XOnlyPublicKey);
/// Custum scalar signing part type
#[derive(Display, From)]
pub(super) struct SigningScalar(Scalar);
/// Custum signature type
#[derive(From)]
pub(super) struct OracleSignature(NoncePoint, SigningScalar);

impl From<OracleSignature> for (NoncePoint, SigningScalar) {
    /// Split signature into public nonce and scalar parts
    fn from(sig: OracleSignature) -> Self {
        (sig.0, sig.1)
    }
}

impl From<OracleSignature> for Signature {
    fn from(sig: OracleSignature) -> Self {
        Signature::from_str((sig.0 .0.to_string() + sig.1.to_string().as_str()).as_str())
            .expect("Nonce and scalar are 64 bytes long")
    }
}

impl From<Signature> for OracleSignature {
    fn from(sig: Signature) -> Self {
        let (x_nonce_bytes, scalar_bytes) = sig.as_ref().split_at(32);
        let scalar_array = scalar_bytes
            .try_into()
            .expect("Schnorr signature is 64 bytes long");
        (
            XOnlyPublicKey::from_slice(x_nonce_bytes)
                .expect("signature split correctly")
                .into(),
            Scalar::from_be_bytes(scalar_array)
                .expect("signature scalar is always less then curve order")
                .into(),
        )
            .into()
    }
}

/// Decompose numerical outcome into base 2 and convert into vec of string
/// digits is the vec length or number of digits used iun total
/// precision is the number of digits dedicated to fractionnal part
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
            outstanding_sk_nonce,
        ),
    )
}

#[cfg(test)]
pub(super) mod test {
    use secp256k1_zkp::{
        schnorr::Signature, All, KeyPair, Message, Secp256k1, UpstreamError, XOnlyPublicKey,
    };

    use crate::oracle::crypto::{
        to_digit_decomposition_vec, NoncePoint, OracleSignature, SigningScalar,
    };

    fn check_bit_conversion(number: f64, digits: u16, precision: u16, result: Vec<String>) {
        assert_eq!(
            to_digit_decomposition_vec(number, digits, precision),
            result
        )
    }

    #[test]
    fn test_one_one() {
        check_bit_conversion(
            2f64,
            20,
            0,
            "00000000000000000010"
                .chars()
                .map(|c| c.to_string())
                .collect(),
        );
    }
    #[test]
    fn test_realistic_price() {
        check_bit_conversion(
            24156.2f64,
            20,
            0,
            "00000101111001011100"
                .chars()
                .map(|c| c.to_string())
                .collect::<Vec<String>>(),
        );
    }
    #[test]
    fn test_all_one() {
        check_bit_conversion(
            (2_i32.pow(20) - 1) as f64,
            20,
            0,
            "11111111111111111111"
                .chars()
                .map(|c| c.to_string())
                .collect::<Vec<String>>(),
        );
    }
    #[test]
    fn test_precision() {
        check_bit_conversion(
            0.125f64,
            20,
            10,
            "00000000000010000000"
                .chars()
                .map(|c| c.to_string())
                .collect::<Vec<String>>(),
        );
    }

    pub fn check_signature_with_nonce(
        secp: &Secp256k1<All>,
        key: &XOnlyPublicKey,
        outcome: &String,
        outstanding_sk_nonce: Option<&[u8; 32]>,
        result: Signature,
    ) -> Result<(), UpstreamError> {
        if let Some(sk_nonce) = outstanding_sk_nonce {
            let (nonce, _): (NoncePoint, SigningScalar) =
                Into::<OracleSignature>::into(result).into();
            let nonce_pair = KeyPair::from_seckey_slice(secp, sk_nonce).unwrap();
            assert_eq!(nonce_pair.x_only_public_key().0, nonce.0);
        }
        secp.verify_schnorr(
            &result,
            &Message::from_hashed_data::<secp256k1_zkp::hashes::sha256::Hash>(outcome.as_bytes()),
            key,
        )
        // secp.verify_schnorr(
        //     &result,
        //     &Message::from_slice(&[&[0_u8; 31], outcome.as_bytes()].concat()).unwrap(),
        //     key,
        // )
    }
}
