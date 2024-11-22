use derive_more::From;
use dlc::secp_utils::schnorrsig_sign_with_nonce;
use secp256k1_zkp::{
    hashes::{sha256, Hash},
    schnorr::Signature,
    All, Keypair, Message, Scalar, Secp256k1, XOnlyPublicKey,
};

/// We use custom types to implement signature splitting into nonce and scalar.
/// Custom public nonce type
#[derive(From)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub(super) struct NoncePoint(XOnlyPublicKey);
/// Custom scalar signing part type
#[derive(From)]
pub(super) struct SigningScalar(Scalar);
/// Custom signature type
#[derive(From)]
pub(super) struct OracleSignature(pub(super) NoncePoint, pub(super) SigningScalar);

impl From<OracleSignature> for (NoncePoint, SigningScalar) {
    /// Split signature into public nonce and scalar parts
    fn from(sig: OracleSignature) -> Self {
        (sig.0, sig.1)
    }
}

impl From<OracleSignature> for Signature {
    fn from(sig: OracleSignature) -> Self {
        Signature::from_slice(
            [sig.0 .0.serialize(), sig.1 .0.to_be_bytes()]
                .concat()
                .as_slice(),
        )
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
/// precision is the number of digits dedicated to fractional part
pub(super) fn to_digit_decomposition_vec(outcome: f64, digits: u16, precision: u16) -> Vec<String> {
    let outcome_rounded = (outcome * ((2_u64.pow(precision as u32)) as f64)).round() as u64;
    let outcome_binary = format!("{:0width$b}", outcome_rounded, width = digits as usize);
    outcome_binary
        .chars()
        .map(|char| char.to_string())
        .collect::<Vec<_>>()
}

pub(super) fn sign_outcome(
    secp: &Secp256k1<All>,
    key_pair: &Keypair,
    outcome: &String,
    outstanding_sk_nonce: &[u8; 32],
) -> (String, Signature) {
    (
        outcome.to_owned(),
        schnorrsig_sign_with_nonce(
            secp,
            &Message::from_digest(*sha256::Hash::hash(outcome.as_bytes()).as_ref()),
            key_pair,
            outstanding_sk_nonce,
        ),
    )
}

#[cfg(test)]
pub(super) mod test {
    use std::str::FromStr;

    use secp256k1_zkp::{
        hashes::{hex::FromHex, sha256, Hash},
        schnorr::Signature,
        All, Keypair, Message, Secp256k1, UpstreamError, XOnlyPublicKey,
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
    fn test_digit_decomposition() {
        check_bit_conversion(
            2f64,
            20,
            0,
            "00000000000000000010"
                .chars()
                .map(|c| c.to_string())
                .collect(),
        );
        check_bit_conversion(
            24156.2f64,
            20,
            0,
            "00000101111001011100"
                .chars()
                .map(|c| c.to_string())
                .collect::<Vec<String>>(),
        );
        check_bit_conversion(
            (2_i32.pow(20) - 1) as f64,
            20,
            0,
            "11111111111111111111"
                .chars()
                .map(|c| c.to_string())
                .collect::<Vec<String>>(),
        );
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
            let nonce_pair = Keypair::from_seckey_slice(secp, sk_nonce).unwrap();
            assert_eq!(nonce_pair.x_only_public_key().0, nonce.0);
        }
        secp.verify_schnorr(
            &result,
            &Message::from_digest(*sha256::Hash::hash(outcome.as_bytes()).as_ref()),
            key,
        )
        // secp.verify_schnorr(
        //     &result,
        //     &Message::from_slice(&[&[0_u8; 31], outcome.as_bytes()].concat()).unwrap(),
        //     key,
        // )
    }

    pub fn check_signature_with_tag(
        secp: &Secp256k1<All>,
        key: &XOnlyPublicKey,
        msg: impl AsRef<[u8]>,
        tag: &str,
        result: Signature,
    ) -> Result<(), UpstreamError> {
        let tag_hash = <secp256k1_zkp::hashes::sha256::Hash>::hash(tag.as_bytes());
        let tag = tag_hash.as_ref();
        let payload = [tag, tag, msg.as_ref()].concat();
        secp.verify_schnorr(
            &result,
            &Message::from_digest(*sha256::Hash::hash(payload.as_ref()).as_ref()),
            key,
        )
        // secp.verify_schnorr(
        //     &result,
        //     &Message::from_slice(&[&[0_u8; 31], outcome.as_bytes()].concat()).unwrap(),
        //     key,
        // )
    }

    fn test_bip340_conversion(
        secp: &Secp256k1<All>,
        sig_str: &str,
        msg_hex: &str,
        pub_key_str: &str,
        nonce_secret_str: Option<&str>,
    ) {
        // Check if passing bip340 test vec
        let sig_secp = Signature::from_str(sig_str).unwrap();
        let zero_msg = Message::from_digest(<[u8; 32]>::from_hex(msg_hex).unwrap());
        let pub_key = XOnlyPublicKey::from_str(pub_key_str).unwrap();
        secp.verify_schnorr(&sig_secp, &zero_msg, &pub_key).unwrap();

        // Convert into nonce and scalar

        let oracle_sig = OracleSignature::from(sig_secp);
        let (oracle_nonce, oracle_scalar) = oracle_sig.into();

        // If secret nonce is given, check that it match public one in signature

        match nonce_secret_str {
            Some(secret_str) => assert_eq!(
                &NoncePoint(
                    Keypair::from_seckey_str(secp, secret_str)
                        .unwrap()
                        .x_only_public_key()
                        .0
                ),
                &oracle_nonce
            ),
            None => {}
        }

        // Aggregate back into a signature
        let oracle_sig: OracleSignature = (oracle_nonce, oracle_scalar).into();

        // Check if Signature if still valid after manipulations
        secp.verify_schnorr(&oracle_sig.into(), &zero_msg, &pub_key)
            .unwrap();
    }

    fn hash_zero() -> String {
        hex::encode::<[u8; 32]>(*secp256k1_zkp::hashes::sha256::Hash::hash("0".as_bytes()).as_ref())
    }
    fn hash_one() -> String {
        hex::encode::<[u8; 32]>(*secp256k1_zkp::hashes::sha256::Hash::hash("1".as_bytes()).as_ref())
    }

    #[test]
    fn test_vec_bip340_conversion() {
        let secp = Secp256k1::new();
        test_bip340_conversion(&secp, "e907831f80848D1069a5371b402410364BDF1C5F8307B0084C55F1CE2DCA821525F66A4A85EA8B71E482A74F382D2CE5EBEEE8FDB2172F477DF4900D310536C0", "0000000000000000000000000000000000000000000000000000000000000000", "F9308A019258C31049344F85F89D5229B531C845836F99B08601F113Bce036f9", None);
        test_bip340_conversion(&secp, "6896BD60EEAE296DB48A229FF71DFE071BDE413E6D43F917DC8DCF8C78DE33418906D11AC976ABCCB20B091292BFF4EA897EFCB639EA871CFA95F6DE339E4B0A", "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89", "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659", None);
        test_bip340_conversion(&secp, "00000000000000000000003B78CE563F89A0ED9414F5AA28AD0D96D6795F9C6376AFB1548AF603B3EB45C9F8207DEE1060CB71C04E80F593060B07D28308D7F4", "4DF3C3F68FCC83B27E9D42C90431A72499F17875C81A599B566C9889B9696703", "D69C3509BB99E412E68B0FE8544E72837DFA30746D8BE2AA65975F29D22DC7B9", None);
        test_bip340_conversion(&secp, "ff19c598971a495aed75a3b2f8bcdac124fc4f7491ce23124052656321baf5301eb081c1da3ca58bd631ed60ae9dded44071e88aca8ddf6dacf646cf02f85d74", &hash_zero(), "ce4b7ad2b45de01f0897aa716f67b4c2f596e54506431e693f898712fe7e9bf3", None);
        test_bip340_conversion(&secp, "7a43247533372a1ea782dff57f3c691c7e2dcd8b8ec6c57d59977f7101785bb40666a8b521034ce010130ba3d8fb1e358074e3a3854a8f7e31da8a8a41ec6c89", &hash_zero(), "24653eac434488002cc06bbfb7f10fe18991e35f9fe4302dbea6d2353dc0ab1c", None);
        test_bip340_conversion(&secp, "b9f70047b2fe621b2b4af36abc3cd4e0d155dcc2f300a8238d48108a4a1d5510283c2058d7210737f2baf60bd05d98f86ebb832d108f08455ffea58fda9b7283", &hash_zero(), "24653eac434488002cc06bbfb7f10fe18991e35f9fe4302dbea6d2353dc0ab1c", None);
        test_bip340_conversion(&secp, "49a2c17b1eb2bcfb08d90728f301e8f80d6296cb7964129312c769cf7dd1942761cd6df12fe1d88fa5628042e676c4deaaf2aef07e516a8efdd2a4e2fa397159", &hash_one(), "24653eac434488002cc06bbfb7f10fe18991e35f9fe4302dbea6d2353dc0ab1c", None);
    }
}
