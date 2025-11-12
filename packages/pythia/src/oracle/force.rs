use chrono::{DateTime, Utc};
use secp256k1_zkp::{
    rand::{RngCore, thread_rng},
    schnorr::Signature,
};

use crate::{
    DBconnection, SECP,
    data_models::{
        asset_pair::AssetPair,
        event_ids::EventIdInfos,
        oracle_msgs::{Announcement, Attestation, Event},
    },
    oracle::{
        Oracle, ScalarsRecords, announcing::SignedEventToInsert, error::Result, sign_event,
        sign_outcome, to_digit_decomposition_vec,
    },
};

impl Oracle {
    pub async fn force_new_attest_with_price(
        &self,
        db: &DBconnection,
        maturation: DateTime<Utc>,
        price: f64,
    ) -> Result<(Announcement, Attestation)> {
        let event_id_infos =
            EventIdInfos::spot_from_pair_and_timestamp(AssetPair::default(), maturation);
        let event_id = event_id_infos.as_event_id();
        let event = &self.asset_pair_info.event_descriptor;
        let digits = event.nb_digits;
        let (nonces_keypairs, maybe_announcement) = match db.get_event(event_id).await? {
            Some(postgres_response) => match &postgres_response.scalars_records {
                ScalarsRecords::DigitsSkNonce(sk_nonces) => {
                    info!(
                        "!!! Forced announcement !!!: {event_id} event is already announced, will attest it immediately with price outcome {price}"
                    );
                    let as_keypairs = sk_nonces
                        .iter()
                        .map(|sk| {
                            secp256k1_zkp::Keypair::from_seckey_slice(&SECP, sk)
                                .expect("too low probability of secret to be invalid")
                        })
                        .collect::<Vec<_>>();
                    (
                        as_keypairs,
                        Some(self.compute_announcement(postgres_response)),
                    )
                }
                ScalarsRecords::DigitsAttestations(outcome, _) => {
                    info!(
                        "!!! Forced attestation !!!: {event_id} event is already attested with price {outcome}, ignore forcing"
                    );
                    let (oracle_announcement, oracle_attestation) = self
                        .oracle_state(db, event_id)
                        .await?
                        .expect("is announced");

                    return Ok((
                        oracle_announcement,
                        oracle_attestation.expect("is attested"),
                    ));
                }
            },
            None => {
                let mut nonces_keypairs = Vec::with_capacity(digits.into());
                debug!(
                    "!!! Forced announcement !!!: created oracle event and announcement with maturation {maturation}"
                );
                // Begin scope to ensure ThreadRng is drop at compile time so that Oracle derive Send AutoTrait
                {
                    let mut rng = thread_rng();
                    let mut secret_bytes = [0u8; 32];
                    for _ in 0..digits {
                        rng.fill_bytes(&mut secret_bytes);
                        let oracle_r_kp =
                            secp256k1_zkp::Keypair::from_seckey_slice(&SECP, &secret_bytes)
                                .expect("secret_bytes has the required length");
                        nonces_keypairs.push(oracle_r_kp);
                    }
                }; // End scope: ThreadRng is drop at compile time so that Oracle derives Send AutoTrait
                (nonces_keypairs, None)
            }
        };

        let outcomes = to_digit_decomposition_vec(
            price,
            digits,
            event
                .precision
                .try_into()
                .expect("Number range good in forced case"),
        );

        let signatures = outcomes
            .iter()
            .zip(nonces_keypairs.iter())
            .map(|(outcome, nonce_keypair)| {
                sign_outcome(&self.keypair, *outcome, &nonce_keypair.secret_bytes())
            })
            .collect::<Vec<Signature>>();

        let announcement = if let Some(announcement) = maybe_announcement {
            announcement
        } else {
            let oracle_event = Event {
                oracle_nonces: nonces_keypairs
                    .iter()
                    .map(|kp| kp.x_only_public_key().0.serialize())
                    .collect(),
                maturity: maturation.timestamp() as u32,
                event_descriptor: self.asset_pair_info.event_descriptor,
                event_id,
            };

            let event_to_insert = SignedEventToInsert {
                announcement_signature: sign_event(&self.keypair, &oracle_event),
                nonces_keypairs,
                maturity: oracle_event.maturity,
                event_id_infos,
                event_descriptor: oracle_event.event_descriptor,
            };
            db.insert_announcement(&event_to_insert).await?;

            event_to_insert.as_announcement(self.get_public_key())
        };

        let attestation = Attestation {
            event_id,
            oracle_public_key: self.keypair.public_key().into(),
            signatures,
            outcomes,
        };

        info!(
            "!!! Forced attestation !!!: attested from announcement {:?} with price outcome {}, giving the following attestation: {:?}",
            &announcement, price, attestation
        );
        db.update_to_attestation(&attestation, price).await?;

        Ok((announcement, attestation))
    }
}
