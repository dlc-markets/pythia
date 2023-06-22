use std::str::FromStr;

use displaydoc::Display;

use dlc::secp_utils::schnorrsig_sign_with_nonce;
use secp256k1_zkp::{
    hashes::sha256, schnorr::Signature, All, KeyPair, Message, Scalar, Secp256k1, XOnlyPublicKey,
};
/// Custum signature type
pub(super) struct OracleSignature(pub(super) Signature);
/// Custum public nonce type
pub(super) struct NoncePoint(pub(super) XOnlyPublicKey);
/// Custum scalar signing part type
#[derive(Display)]
pub(super) struct SigningScalar(pub(super) Scalar);

impl From<OracleSignature> for (NoncePoint, SigningScalar) {
    /// Split signature into public nonce and scalar parts
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
    /// Join associated nonce and scalar parts tuple into a signature
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
    use std::str::FromStr;

    use dlc_messages::oracle_msgs::OracleAttestation;
    use secp256k1_zkp::{
        hashes::sha256, schnorr::Signature, All, KeyPair, Message, Secp256k1, UpstreamError,
        XOnlyPublicKey,
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
            let (nonce, _): (NoncePoint, SigningScalar) = OracleSignature(result).into();
            let nonce_pair = KeyPair::from_seckey_slice(secp, sk_nonce).unwrap();
            assert_eq!(nonce_pair.x_only_public_key().0, nonce.0);
        }
        secp.verify_schnorr(
            &result,
            &Message::from_hashed_data::<sha256::Hash>(outcome.as_bytes()),
            key,
        )
    }

    #[test]
    fn test_vec() {
        let attestation_test_vec = OracleAttestation {
            oracle_public_key: XOnlyPublicKey::from_str("10dc8cf51ae3ee1c7967ffb9c9633a5ab06206535d8e1319f005a01ba33bc05d").unwrap(),
            signatures: ["ee05b1211d5f974732b10107dd302da062be47cd18f061c5080a50743412f9fd590cad90cfea762472e6fe865c4223bd388c877b7881a27892e15843ff1ac360","59ab83597089b48f5b3c2fd07c11edffa6b1180bdb6d9e7d6924979292d9c53fe79396ceb0782d5941c284d1642377136c06b2d9c2b85bda5969a773a971b5b0","d1f8c31a83bb34433da5b9808bb3692dd212b9022b7bc8f269fc817e96a7195db18262e934bebd4e68a3f2c96550826a5530350662df4c86c004f5cf1121ca67","e5cec554c39c4dd544d70175128271eecad77c1e3eaa6994c657e257d5c1c9dcd19b041ea8030e75448245b7f91705ad914c32761671a6172f928904b439ea6b","a209116d20f0931113c0880e8cd22d3f003609a32322ff8df241ef16e7d4efd1a9b723f582a22073e21188635f09f41f270f3126014542861be14b62b09c0ecc","f1da0b482f08f545a92338392b71cec33d948a5e5732ee4d5c0a87bd6b6cc12feeb1498da7afd93ae48ec4ce581ee79c0e92f338d3777c2ef06578e4ec1a853c","d9ab68244a3b47cc8cbd5a972f2f5059fc6b9711dba8d4a7a23607a99b9655593bab3abc1d3b02402cd0809c3c7016c741742efb363227de2bcfdcf290a053b3","c1146c1767a947f77794d05a2f58e50af824e3c8d70adde883e58d2dc1ddb157323b0aaf8cfb5b076a12395756bdcda64ab5d4799e43c88a41993659e6d49471","0d29d9383c9ee41055e1cb40104c9ca75280162779c0162cb6bf9aca2b223aba17de4b3f0f29ae6b749f22ba467b7e9f05456e8abb3ec328f62b7a924c6d4828","2bcc54002ceb271a940f24bc6dd0562b99c2d76cfb8f145f42ac37bc34fd3e94adba1194c5be91932b818c5715c73f287e066e228d796a373c4aec67fd777070","a91f77e3435c577682ff744d6f7da66c865a42e8645276dedf2ed2b8bc4c80285dff4b553b2231592e0fa8b4f242acb6888519fe82c457cc5204e5d9d511303a","546409d6bcdcfd5bef39957c8b1b09f7805b08ec2311bc73cf6927ae11f3567ffe8428aa7faa661518e9c02a702212ab05e494aab84624c3dd1a710f8c4c369b","9d601ee8a3d28dcdfdd05581f1b24d6e5a576f0b5544eb7c9921cb87a23fdb293c1edca89b43b5b84c1e305fbe52facbe6b03575aed8f95b4faccc90e0eb45ef","636b8028e9cd6cba6be5b3c1789b62aecfc17e9c28d7a621cfad2c3cf751046528028e1dbd6cee050d5d570cf5a3d8986471d73e7edca4093e36fc8e1097fb65","57c6337b52dc7fd8f49b29105f168fc9b4cb88ed2ba5f0e9a80a21e20836f87f875c3fe92afb437dd5647630b54eda6ba1be76ba6df8b641eb2e8be8ff1182dc","9e8843e32f9de4cd6d5bb9e938fd014babe11bb1faf35fc411d754259bc374f34dd841ed91f6bb3f030bc55a4791cdc41471c33b3f05fd35b9d1768fd381f953","97da4963747ab5e50534b93274065cba4fd24e6b7a9d3310db2596af24f70961fb03535e2a5ae272f7ea14e86daafa57073631596fecf7ceadf4ae3e6941b69e","94a414569743f87f1462a503be8cff1f229096d190b8b1349519c612b74eea872d5d763570aaaa54fad0605a43d742203bce489deea5570750030191e293c253","4d7117b89aad73eca7b341749bd54ffdd459b9b8b4ff128344d09273f66a3d2c01d2c86b61f7642d6e81f488580b456685cd68660458cff83b8858a05c9a1f4d","b12153a393a4fddac3079c1878cb89afccfe0ac8f539743c0608049f445e49ac7c89e33fcf832cda8d7e8a4f4dae94a303170f16c697feed8b78015873bd5ffc"].into_iter().map(|s| Signature::from_str(s).unwrap()).collect(),
            outcomes: ["0","0","0","0","0","1","1","1","0","1","0","0","0","0","1","1","1","0","1","0"].into_iter().map(|d| d.to_string()).collect(),
        };
        let secp = Secp256k1::new();
        attestation_test_vec
            .signatures
            .into_iter()
            .zip(&attestation_test_vec.outcomes)
            .map(|(sig, dig)| {
                check_signature_with_nonce(
                    &secp,
                    &attestation_test_vec.oracle_public_key,
                    dig,
                    None,
                    sig,
                )
            })
            .for_each(|res| {
                if let Err(e) = res {
                    println!("Error: {:?}", e);
                    panic!("A signature is invalid")
                }
            });
    }
}
