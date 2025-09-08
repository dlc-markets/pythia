use secp256k1_zkp::{
    hashes::{sha256, Hash},
    schnorr::Signature,
    All, Keypair, Message, Scalar, Secp256k1, UpstreamError,
};

use crate::{
    data_models::{oracle_msgs::Event, Outcome, OUTCOME_ONE, OUTCOME_ZERO},
    SECP,
};

/// Custom signature type
pub(super) struct OracleSignature {
    nonce: [u8; 32],
    scalar: Scalar,
}

impl From<(Scalar, [u8; 32])> for OracleSignature {
    fn from((scalar, nonce): (Scalar, [u8; 32])) -> Self {
        Self { nonce, scalar }
    }
}

impl TryFrom<OracleSignature> for Signature {
    type Error = UpstreamError;

    fn try_from(sig: OracleSignature) -> Result<Self, Self::Error> {
        Signature::from_slice(&[sig.nonce, sig.scalar.to_be_bytes()].concat())
    }
}

impl From<Signature> for OracleSignature {
    fn from(sig: Signature) -> Self {
        let (x_nonce_bytes, scalar_bytes) = sig.as_ref().split_at(32);
        let scalar_array = scalar_bytes
            .try_into()
            .expect("Schnorr signature is 64 bytes long");
        Self {
            nonce: x_nonce_bytes.try_into().expect("signature split correctly"),
            scalar: Scalar::from_be_bytes(scalar_array)
                .expect("signature scalar is always less then curve order"),
        }
    }
}

pub(super) fn sign_event(key_pair: &Keypair, event: &Event) -> Signature {
    let message = {
        let mut hash_engine = sha256::HashEngine::default();
        event
            .write_to(&mut hash_engine)
            .expect("write to hash engine cannot fail");
        Message::from_digest(sha256::Hash::from_engine(hash_engine).to_byte_array())
    };

    SECP.sign_schnorr(&message, key_pair)
}

/// SHA 256 hash of the string "0"
const HASH_ZERO_BYTES: [u8; 32] = [
    95, 236, 235, 102, 255, 200, 111, 56, 217, 82, 120, 108, 109, 105, 108, 121, 194, 219, 194, 57,
    221, 78, 145, 180, 103, 41, 215, 58, 39, 251, 87, 233,
];

/// SHA 256 hash of the string "1"
const HASH_ONE_BYTES: [u8; 32] = [
    107, 134, 178, 115, 255, 52, 252, 225, 157, 107, 128, 78, 255, 90, 63, 87, 71, 173, 164, 234,
    162, 47, 29, 73, 192, 30, 82, 221, 183, 135, 91, 75,
];

/// Decompose numerical outcome into base 2 and convert into vec of string
/// digits is the vec length or number of digits used iun total
/// precision is the number of digits dedicated to fractional part
pub(super) fn to_digit_decomposition_vec(
    outcome: f64,
    digits: u16,
    precision: u16,
) -> Vec<Outcome> {
    let outcome_rounded = (outcome * (1 << precision) as f64).round() as u64;

    if u64::checked_ilog2(outcome_rounded) > Some(digits as u32) {
        return vec![Outcome::try_from('1').expect("1 is a valid outcome"); digits as usize];
    }

    let outcome_zero = OUTCOME_ZERO.with(|outcome| **outcome);
    let outcome_one = OUTCOME_ONE.with(|outcome| **outcome);

    (0..digits)
        .map(|i| {
            if ((outcome_rounded >> i) & 1) == 1 {
                outcome_one
            } else {
                // if not 1 must be 0 because of "& 1"
                outcome_zero
            }
        })
        .rev()
        .collect()
}

pub(super) fn sign_outcome(
    secp: &Secp256k1<All>,
    key_pair: &Keypair,
    outcome: Outcome,
    outstanding_sk_nonce: &[u8; 32],
) -> Signature {
    // See `sign_with_nonce` module for more details.
    use sign_with_nonce::schnorrsig_sign_with_nonce;

    // The oracle will often only sign "0" and "1" by design
    // so we use precomputed hash in those cases to speed it up
    match outcome.as_ref() {
        "0" => schnorrsig_sign_with_nonce(
            secp,
            &Message::from_digest(HASH_ZERO_BYTES),
            key_pair,
            outstanding_sk_nonce,
        ),
        "1" => schnorrsig_sign_with_nonce(
            secp,
            &Message::from_digest(HASH_ONE_BYTES),
            key_pair,
            outstanding_sk_nonce,
        ),
        other => schnorrsig_sign_with_nonce(
            secp,
            &Message::from_digest(sha256::Hash::hash(other.as_bytes()).to_byte_array()),
            key_pair,
            outstanding_sk_nonce,
        ),
    }
}

/// This module replicates the implementation of [schnorrsig_sign_with_nonce](https://github.com/p2pderivatives/rust-dlc/blob/v0.7.1/dlc/src/secp_utils.rs#L31)
/// to avoid importing the whole `dlc` crate as dependency just for signing outcomes using the nonce draw at announcement time.
///
/// Because it uses the extern functions of the original C library in [secp256k1-zkp-sys](https://docs.rs/secp256k1-zkp-sys/0.10.1/secp256k1_zkp_sys/),
/// `unsafe` is required in this module. Any unsoundness of its use here must also be reported in the `rust-dlc` repository.
///
/// The code of this module is and must stay a copy paste of the original implementation in `rust-dlc` with only formatting changes.
mod sign_with_nonce {
    use secp256k1_zkp::{
        constants::MESSAGE_SIZE, schnorr::Signature, Keypair, Message, Secp256k1, Signing,
    };
    use secp256k1_zkp_sys::{
        secp256k1_schnorrsig_sign_custom,
        types::{c_int, c_uchar, c_void, size_t},
        CPtr, SchnorrSigExtraParams,
    };

    /// SAFETY: this function must only be used as `SchnorrNonceFn` in SchnorrSigExtraParams.
    extern "C" fn constant_nonce_fn(
        nonce32: *mut c_uchar,
        _msg32: *const c_uchar,
        _msg_len: size_t,
        _key32: *const c_uchar,
        _x_only_pk32: *const c_uchar,
        _algo16: *const c_uchar,
        _algo_len: size_t,
        data: *mut c_void,
    ) -> c_int {
        // SAFETY: data and nonce32 are provided by the secp256k1-zkp-sys crate internals
        // and the nonce data is 32 bytes long.
        unsafe {
            std::ptr::copy_nonoverlapping(data as *const c_uchar, nonce32, 32);
        }
        1
    }

    /// Sign a message for the given keypair using the provided nonce secret.
    pub(super) fn schnorrsig_sign_with_nonce<S: Signing>(
        secp: &Secp256k1<S>,
        msg: &Message,
        keypair: &Keypair,
        nonce: &[u8; 32],
    ) -> Signature {
        let mut sig = [0u8; secp256k1_zkp::constants::SCHNORR_SIGNATURE_SIZE];
        let extra_params =
            SchnorrSigExtraParams::new(Some(constant_nonce_fn), nonce.as_c_ptr() as *const c_void);

        // SAFETY: we cast only safe Rust types into appropriate raw pointer data,
        // all taken reference are dropped in the block, and we provide the correct message size.
        let result_code = unsafe {
            secp256k1_schnorrsig_sign_custom(
                secp.ctx().as_ref(),
                sig.as_mut_c_ptr(),
                msg.as_c_ptr(),
                MESSAGE_SIZE,
                keypair.as_c_ptr(),
                &extra_params,
            )
        };

        assert_eq!(1, result_code);

        Signature::from_slice(&sig).unwrap()
    }
}
#[cfg(test)]
pub(super) mod test {
    use std::str::FromStr;

    use secp256k1_zkp::{
        hashes::{hex::FromHex, sha256, Hash},
        schnorr::Signature,
        All, Keypair, Message, Secp256k1, UpstreamError, XOnlyPublicKey,
    };

    use crate::{
        data_models::Outcome,
        oracle::crypto::{to_digit_decomposition_vec, OracleSignature},
    };

    use super::{HASH_ONE_BYTES, HASH_ZERO_BYTES};

    fn check_bit_conversion(number: f64, digits: u16, precision: u16, result: Vec<Outcome>) {
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
                .map(Outcome::try_from)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        );
        check_bit_conversion(
            24156.2f64,
            20,
            0,
            "00000101111001011100"
                .chars()
                .map(Outcome::try_from)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        );
        check_bit_conversion(
            (2_i32.pow(20) - 1) as f64,
            20,
            0,
            "11111111111111111111"
                .chars()
                .map(Outcome::try_from)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        );
        check_bit_conversion(
            0.125f64,
            20,
            10,
            "00000000000010000000"
                .chars()
                .map(Outcome::try_from)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
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
            let OracleSignature { nonce, scalar: _ } = result.into();
            let nonce_pair = Keypair::from_seckey_slice(secp, sk_nonce).unwrap();
            assert_eq!(nonce_pair.x_only_public_key().0.serialize(), nonce);
        }
        secp.verify_schnorr(
            &result,
            &Message::from_digest(sha256::Hash::hash(outcome.as_bytes()).to_byte_array()),
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
            &Message::from_digest(sha256::Hash::hash(payload.as_ref()).to_byte_array()),
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
        let OracleSignature { nonce, scalar } = oracle_sig;

        // If secret nonce is given, check that it match public one in signature

        if let Some(secret_str) = nonce_secret_str {
            assert_eq!(
                &Keypair::from_seckey_str(secp, secret_str)
                    .unwrap()
                    .x_only_public_key()
                    .0
                    .serialize(),
                &nonce
            )
        };

        // Aggregate back into a signature
        let oracle_sig = OracleSignature { nonce, scalar };

        // Check if Signature if still valid after manipulations
        secp.verify_schnorr(&oracle_sig.try_into().unwrap(), &zero_msg, &pub_key)
            .unwrap();
    }

    fn hash_zero() -> Box<str> {
        hex::encode::<[u8; 32]>(HASH_ZERO_BYTES).into_boxed_str()
    }
    fn hash_one() -> Box<str> {
        hex::encode::<[u8; 32]>(HASH_ONE_BYTES).into_boxed_str()
    }

    #[test]
    fn hash_0_1_check() {
        assert_eq!(
            secp256k1_zkp::hashes::sha256::Hash::hash(&['0' as u8]).to_byte_array(),
            HASH_ZERO_BYTES
        );
        assert_eq!(
            secp256k1_zkp::hashes::sha256::Hash::hash(&['1' as u8]).to_byte_array(),
            HASH_ONE_BYTES
        );
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
