use chrono::{DateTime, Utc};
use futures::{StreamExt as _, stream};
use futures_buffered::BufferedStreamExt as _;
use secp256k1_zkp::schnorr::Signature;

use crate::{
    DBconnection,
    data_models::{
        event_ids::{EventId, EventIdInfos},
        oracle_msgs::Attestation,
    },
    db::EventFromPostgres,
    oracle::{
        Oracle, ScalarsRecords,
        crypto::{join_sig, sign_outcome, to_digit_decomposition_vec},
        error::{OracleError, Result},
    },
};

mod insert_attestation;

struct AttestSessionData {
    digits: u16,
    precision: u16,
    event_id_infos: Option<EventIdInfos>,
    maturity: DateTime<Utc>,
    outstanding_sk_nonces: Box<[[u8; 32]]>,
    outcome: Option<f64>,
}

impl Oracle {
    /// Attest or return attestation of event with given eventID. Return None if it was not announced, a PriceFeeder error if it the outcome is not available.
    /// Store in DB and return some oracle attestation if event is attested successfully.
    pub(crate) async fn try_attest_events(
        &self,
        db: &DBconnection,
        events_id: &[EventId],
    ) -> Result<Vec<Result<Attestation>>> {
        let events = db.get_events_with(events_id).await?;

        assert!(
            events
                .iter()
                .all(|e| e.event_id_infos.asset_pair == self.asset_pair_info.asset_pair),
            "Some events are not related to the oracle that try to attest them: all asset pair must be {}",
            self.asset_pair_info.asset_pair
        );

        self.attest_from_postgres_events(db, events).await
    }

    /// Attest or return attestations of all events at given date. Failing to attest an event will silently skip it.
    /// Store in DB and return its oracle attestation if an event is attested successfully.
    pub(crate) async fn attest_at_date(
        &self,
        db: &DBconnection,
        date: DateTime<Utc>,
    ) -> Result<Vec<Result<Attestation>>> {
        let in_postgres_events = db.get_events_with(date).await?;

        self.attest_from_postgres_events(db, in_postgres_events)
            .await
    }

    async fn attest_from_postgres_events(
        &self,
        db: &DBconnection,
        postgres_events: Vec<EventFromPostgres>,
    ) -> Result<Vec<Result<Attestation>>> {
        let mut attestations_already_available = Vec::with_capacity(postgres_events.len());

        let mut events_to_attest = postgres_events
            .into_iter()
            .filter_map(|event| match event.scalars_records {
                ScalarsRecords::DigitsSkNonce(outstanding_sk_nonces) => Some(AttestSessionData {
                    digits: event.digits,
                    precision: event.precision,
                    event_id_infos: Some(event.event_id_infos),
                    maturity: event.maturity,
                    outstanding_sk_nonces,
                    outcome: None,
                }),
                ScalarsRecords::DigitsAttestations(outcome, scalars) => {
                    attestations_already_available.push(Attestation {
                        event_id: event.event_id_infos.as_event_id(),
                        oracle_public_key: self.keypair.public_key().into(),
                        signatures: event
                            .nonces_public
                            .into_iter()
                            .zip(scalars)
                            .map(|(nonce, scalar)| join_sig(nonce, scalar))
                            .collect(),
                        outcomes: to_digit_decomposition_vec(
                            outcome,
                            event.digits,
                            event.precision,
                        ),
                    });
                    None
                }
            })
            .collect::<Vec<_>>();

        if events_to_attest.is_empty()
            && attestations_already_available.len() == attestations_already_available.capacity()
        {
            return Ok(attestations_already_available.into_iter().map(Ok).collect());
        }

        let mut price_retrieval_results = self
            .asset_pair_info
            .pricefeed
            .retrieve_prices(
                events_to_attest
                    .iter_mut()
                    .map(|e| {
                        e.event_id_infos
                            .take()
                            .expect("event_id_infos is always Some")
                    })
                    .collect::<Vec<_>>(),
            )
            .await?
            .into_iter();

        let mut result = stream::iter(
            events_to_attest
                .extract_if(.., |data| {
                    let (event_id_infos, price) = price_retrieval_results
                        .next()
                        .expect("price retrieval results must be available for each event");
                    data.event_id_infos = Some(event_id_infos);
                    data.outcome = price;
                    price.is_some()
                })
                .map(async |data| {
                    trace!("retrieving price feed for attestation");

                    let outcome = data.outcome.expect("only extracted if some");
                    let event_id_infos = data
                        .event_id_infos
                        .expect("was given back in extract_if closure");

                    let outcomes = to_digit_decomposition_vec(outcome, data.digits, data.precision);
                    let signatures = outcomes
                        .iter()
                        .copied()
                        .zip(data.outstanding_sk_nonces.into_iter())
                        .map(|(outcome, outstanding_sk_nonce)| {
                            sign_outcome(&self.keypair, outcome, &outstanding_sk_nonce)
                        })
                        .collect::<Vec<Signature>>();

                    debug!(
                        "created oracle attestation with maturation {}",
                        data.maturity
                    );

                    let attestation = Attestation {
                        event_id: event_id_infos.as_event_id(),
                        oracle_public_key: self.keypair.public_key().into(),
                        signatures,
                        outcomes,
                    };

                    trace!("attestation {attestation:#?}");

                    db.update_to_attestation(&attestation, outcome).await?;
                    Ok(attestation)
                }),
        )
        .buffered_unordered(2)
        .collect::<Vec<_>>()
        .await;

        result.extend(attestations_already_available.into_iter().map(Ok));
        result.extend(events_to_attest.into_iter().map(|event| {
            Err(OracleError::MissingEventId(
                event.event_id_infos.expect("only extracted if some"),
            ))
        }));

        Ok(result)
    }
}
