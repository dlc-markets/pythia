use dlc_messages::oracle_msgs::{EventDescriptor, OracleAnnouncement, OracleAttestation};
use futures;
use log::info;
use secp256k1_zkp::{schnorr::Signature, Scalar, XOnlyPublicKey};
use sqlx::{
    postgres::{PgPool, PgPoolOptions},
    Result,
};
use std::str::FromStr;
use time::OffsetDateTime;

struct EventResponse {
    id: String,
    digits: i32,
    precision: i32,
    maturity: OffsetDateTime,
    announcement_signature: Vec<u8>,
    outcome: Option<i32>,
}

struct DigitAnnoncementResponse {
    digit_index: i32,
    nonce_public: Vec<u8>,
    nonce_secret: Option<Vec<u8>>,
}
struct DigitAttestationResponse {
    digit_index: i32,
    nonce_public: Vec<u8>,
    bit: Option<Vec<u8>>,
    signature: Option<Vec<u8>>,
}

#[derive(Clone)]
pub enum ScalarsRecords {
    DigitsSkNonce(Vec<[u8; 32]>),
    DigitsAttestations(u32, Vec<Scalar>),
}

#[derive(Clone)]
pub(super) struct PostgresResponse {
    pub digits: u16,
    pub precision: u16,
    pub maturity: OffsetDateTime,
    pub announcement_signature: Signature,
    pub nonce_public: Vec<XOnlyPublicKey>,
    pub scalars_records: ScalarsRecords,
}
#[derive(Clone)]
pub struct DBconnection(PgPool);

impl DBconnection {
    pub async fn new(url: &str, max_connection: u32) -> Result<Self> {
        Ok(DBconnection(
            PgPoolOptions::new()
                .max_connections(max_connection)
                .connect(url)
                .await?,
        ))
    }

    pub async fn is_empty(&self) -> bool {
        match sqlx::query_as!(EventResponse, "SELECT * FROM oracle.events LIMIT 1")
            .fetch_optional(&self.0)
            .await
            .unwrap()
        {
            Some(_) => false,
            None => true,
        }
    }
    pub(super) async fn insert_announcement(
        &self,
        announcement: &OracleAnnouncement,
        outstanding_sk_nonces: Vec<[u8; 32]>,
    ) -> Result<()> {
        let EventDescriptor::DigitDecompositionEvent(ref digits) = announcement.oracle_event.event_descriptor else {
      return Err(sqlx::Error::TypeNotFound { type_name: "Only DigitDecomposition event type is supported".to_string() })
    };

        sqlx::query!(
            "WITH events AS (
                INSERT INTO oracle.events VALUES ($1, $2, $3, $4, $5)
            )
            INSERT INTO oracle.digits (event_id, digit_index, nonce_public, nonce_secret) (
                SELECT * FROM UNNEST($6::VARCHAR[], $7::INT[], $8::BYTEA[], $9::BYTEA[])
            )
            ",
            &announcement.oracle_event.event_id,
            &(digits.nb_digits as i32),
            digits.precision,
            OffsetDateTime::from_unix_timestamp(
                announcement
                    .oracle_event
                    .event_maturity_epoch
                    .try_into()
                    .unwrap(),
            )
            .unwrap(),
            announcement.announcement_signature.as_ref(),
            &vec![announcement.oracle_event.event_id.to_owned(); digits.nb_digits as usize][..],
            &(0..digits.nb_digits as i32).collect::<Vec<i32>>(),
            &announcement
                .oracle_event
                .oracle_nonces
                .iter()
                .map(|x| x.serialize().to_vec())
                .collect::<Vec<Vec<u8>>>(),
            outstanding_sk_nonces.as_slice(): &[Vec<u8>]
        )
        .execute(&self.0)
        .await?;
        Ok(())
    }

    pub(super) async fn update_to_attestation(
        &self,
        event_id: &str,
        attestation: &OracleAttestation,
        outcome: u32,
    ) -> Result<()> {
        let (indexes, (bits, sigs)): (Vec<usize>, (Vec<Vec<u8>>, Vec<Vec<u8>>)) = attestation
            .outcomes
            .iter()
            .map(|b| vec![u8::from_str(b).unwrap()])
            .zip(
                attestation
                    .signatures
                    .iter()
                    .map(|sig| sig.as_ref().split_at(32).1.to_vec()),
            )
            .enumerate()
            .unzip();
        sqlx::query!(
            "WITH events AS (
                UPDATE oracle.events SET outcome = $1::INT WHERE id = $2::TEXT
            )
            UPDATE oracle.digits
        SET bit = bulk.bit, signature = bulk.sig, signing_ts = NOW(), nonce_secret = NULL
        FROM ( 
            SELECT *
            FROM UNNEST($3::BYTEA[], $4::BYTEA[], $5::VARCHAR[], $6::INT[]) 
            AS t(bit, sig, id, digit)
            ) AS bulk 
        WHERE event_id = bulk.id AND digit_index = bulk.digit
        ",
            &(outcome as i32),
            event_id,
            &bits[..],
            &sigs,
            &vec![event_id.to_owned(); indexes.len() as usize][..],
            &indexes.into_iter().map(|x| x as i32).collect::<Vec<i32>>()[..],
        )
        .execute(&self.0)
        .await?;

        Ok(())
    }

    pub(super) async fn get_event(&self, event_id: &String) -> Result<Option<PostgresResponse>> {
        let Some(event) = sqlx::query_as!(
        EventResponse,
        "SELECT e.* FROM oracle.events e WHERE e.id = $1",
        event_id
    )
    .fetch_optional(&self.0)
    .await? else {return Ok(None)};

        match &event.outcome {
            None => {
                let digits = sqlx::query_as!(
                DigitAnnoncementResponse,
                "SELECT digit_index, nonce_public, nonce_secret FROM oracle.digits WHERE event_id = $1;",
                event_id
            )
            .fetch_all(&self.0)
            .await?;
                let mut aggregated_rows: Vec<(u16, (XOnlyPublicKey, [u8; 32]))> = digits
                    .iter()
                    .map(|x| {
                        (
                            x.digit_index as u16,
                            (
                                XOnlyPublicKey::from_slice(&x.nonce_public).unwrap(),
                                x.nonce_secret
                                    .as_ref()
                                    .unwrap()
                                    .as_slice()
                                    .try_into()
                                    .unwrap(),
                            ),
                        )
                    })
                    .collect();
                aggregated_rows.sort_unstable_by_key(|&d| d.0);
                type AnnouncementRows = (Vec<u16>, (Vec<XOnlyPublicKey>, Vec<[u8; 32]>));
                let (_, (nonce_public, nonce_secret)): AnnouncementRows =
                    aggregated_rows.into_iter().unzip();
                Ok(Some(PostgresResponse {
                    digits: event.digits as u16,
                    precision: event.precision as u16,
                    maturity: event.maturity,
                    announcement_signature: Signature::from_slice(
                        &event.announcement_signature[..],
                    )
                    .unwrap(),
                    nonce_public,
                    scalars_records: ScalarsRecords::DigitsSkNonce(nonce_secret),
                }))
            }
            Some(outcome) => {
                let digits = sqlx::query_as!(
                DigitAttestationResponse,
                "SELECT digit_index, nonce_public, bit, signature FROM oracle.digits WHERE event_id = $1;",
                event_id
            )
            .fetch_all(&self.0)
            .await?;
                type AggregatedRows = (u16, (XOnlyPublicKey, (String, Scalar)));
                let mut aggregated_rows: Vec<AggregatedRows> = digits
                    .iter()
                    .map(|x| {
                        (
                            x.digit_index as u16,
                            (
                                XOnlyPublicKey::from_slice(&x.nonce_public).unwrap(),
                                (
                                    (x.bit.as_ref().unwrap()[0] as i8).to_string(),
                                    Scalar::from_be_bytes(
                                        x.signature
                                            .as_ref()
                                            .unwrap()
                                            .as_slice()
                                            .try_into()
                                            .unwrap(),
                                    )
                                    .unwrap(),
                                ),
                            ),
                        )
                    })
                    .collect();
                aggregated_rows.sort_unstable_by_key(|d| d.0);
                type AttestationRows =
                    (Vec<u16>, (Vec<XOnlyPublicKey>, (Vec<String>, Vec<Scalar>)));
                let (_, (nonce_public, (bits, sigs))): AttestationRows =
                    aggregated_rows.into_iter().unzip();
                Ok(Some(PostgresResponse {
                    digits: event.digits as u16,
                    precision: event.precision as u16,
                    maturity: event.maturity,
                    announcement_signature: Signature::from_slice(
                        &event.announcement_signature[..],
                    )
                    .unwrap(),
                    nonce_public,
                    scalars_records: ScalarsRecords::DigitsAttestations(
                        outcome.clone() as u32,
                        sigs,
                    ),
                }))
            }
        }
    }
}
