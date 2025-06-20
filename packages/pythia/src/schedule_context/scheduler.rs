use chrono::{Duration as ChronoDuration, Utc};
use std::time::Duration;
use tokio::{
    sync::{broadcast::Sender, oneshot::Receiver},
    time::sleep,
};

use super::{error::PythiaContextError, OracleContext};
use crate::{
    api::EventNotification,
    data_models::{event_ids::EventId, oracle_msgs::Attestation},
    error::PythiaError,
    oracle::{error::OracleError, Oracle, CHUNK_SIZE},
};

/// The Scheduler holds a static reference to the running oracles and schedule configuration file with the API
/// Its context also includes the channel sender endpoint to broadcast announcements/attestations to websockets.
pub(crate) struct SchedulerContext<Context: OracleContext> {
    pub(super) oracle_context: Context,
    pub(super) offset_duration: Duration,
    pub(super) channel_sender: Sender<EventNotification>,
    pub(super) error_rx: Receiver<OracleError>,
}

impl<Context: OracleContext> SchedulerContext<Context> {
    pub(super) fn new(
        oracle_context: Context,
        offset_duration: ChronoDuration,
        channel_sender: Sender<EventNotification>,
        error_rx: Receiver<OracleError>,
    ) -> Result<Self, PythiaContextError> {
        let oracle_context_schedule = oracle_context.schedule();
        // This is to prevent a panic produced in start_schedule by reaching "unreachable" marked code
        // The configured cron schedule may not produce a value although it is correctly parsed
        // Using "59 59 23 31 11 * 2100" as cron schedule in config file trigger this error in current cron crate version
        oracle_context_schedule.upcoming(Utc).next().ok_or(
            PythiaContextError::CronScheduleProduceNoValue(Box::new(
                oracle_context_schedule.clone(),
            )),
        )?;

        let offset_duration = offset_duration.to_std()?;
        Ok(Self {
            oracle_context,
            offset_duration,
            channel_sender,
            error_rx,
        })
    }
}

/// Start the scheduler of announcements and attestations using the context made with the api one.
/// It computes a date iterator from cron-like config and spawns a task for each type of event.
/// At each iteration it sleeps if necessary without blocking until the next date produced by the iterator.
pub(crate) async fn start_schedule<Context>(
    mut context: SchedulerContext<Context>,
) -> Result<(), PythiaError>
where
    Context: OracleContext + Clone + Send + Sync + 'static,
{
    let oracle_context = Context::clone(&context.oracle_context);
    let event_tx = context.channel_sender;

    // start event creation tasks for announcements and attestations
    info!("creating oracle events and schedules");

    let cloned_event_tx = event_tx.clone();
    let start_time = Utc::now();

    let announcement_scheduled_dates = oracle_context
        .schedule()
        .after_owned(start_time)
        .map(move |date| date - context.offset_duration);

    let announcement_task = async move {
        // Vector to store maturation dates that need to be processed in batches
        // Used to accumulate announcements when events have already matured or are imminent
        // This batching approach improves performance by reducing database operations
        let mut pending_maturations = Vec::new();

        'announcement_tick: for next_time in announcement_scheduled_dates {
            // Check if there was an error in the previous iteration
            if let Ok(oracle_result) = context.error_rx.try_recv() {
                return Err(PythiaError::Oracle(oracle_result));
            }
            // We compute how much time we may have to sleep before continue
            // Converting into std Duration type fail here if we don't have to sleep
            let maybe_std_duration = (next_time - Utc::now()).to_std();
            let Ok(duration) = maybe_std_duration else {
                // We accumulate announcements in a vector to avoid frequent individual database insertions.
                // When (next_time - Utc::now()) is negative, it means the event has already matured or is
                // imminent, so we continue accumulating announcements rather than inserting them immediately.
                // Once (next_time - Utc::now()) becomes non-negative (future events), we perform a bulk
                // insertion of all accumulated announcements into the PostgreSQL database with a single query
                // for better performance.
                pending_maturations.push(next_time + context.offset_duration);
                continue 'announcement_tick;
            };

            debug!(
                "next announcement at {} in {:?} with maturity {}",
                &next_time,
                &duration,
                next_time + context.offset_duration
            );

            if !pending_maturations.is_empty() {
                info!(
                    "Pending maybe_std_durations size: {:?}",
                    pending_maturations.len()
                );

                let oracle_context = Context::clone(&oracle_context);

                // We spawn a detached task to process missed announcements in the background
                tokio::spawn(async move {
                    for oracle in oracle_context.oracles().values() {
                        // Collect all processed chunks and store any errors in the error channel
                        if let Err(error) = oracle
                            .create_many_announcements::<CHUNK_SIZE>(&pending_maturations)
                            .await
                        {
                            oracle_context.send_error(error);
                        };
                    }
                    info!("Oracle announcements are in sync");
                });

                // Reinitialize the pending maturations vector to avoid borrowing issues
                pending_maturations = Vec::new();
            }

            sleep(duration).await;

            for oracle in oracle_context.oracles().values() {
                oracle
                    .create_announcements_at_date(next_time + context.offset_duration)
                    .await?
                    .into_iter()
                    .take_while(|_| Sender::receiver_count(&cloned_event_tx) != 0)
                    .for_each(|announcement| {
                        cloned_event_tx
                            .send((oracle.asset_pair_info.asset_pair, announcement).into())
                            .expect("usable channel");
                    });
            }
        }
        unreachable!("Cron schedule can be consumed only after 2100")
    };

    let oracle_context = context.oracle_context;
    let attestation_scheduled_dates = oracle_context.schedule().after_owned(start_time);

    let attestation_task = async move {
        for next_time in attestation_scheduled_dates {
            if let Ok(duration) = (next_time - Utc::now()).to_std() {
                debug!("next attestation at {} in {:?}", &next_time, &duration);
                sleep(duration).await;
            };

            'oracles: for oracle in oracle_context.oracles().values() {
                let Ok(attestations) = oracle
                    .attest_at_date(next_time)
                    .await
                    .map_err(|e| error!("The oracle scheduler failed to attest: {}", e))
                else {
                    continue 'oracles;
                };

                'attestations: for maybe_attestation in attestations {
                    let Err(error) = maybe_attestation.map(|attestation| {
                        if Sender::receiver_count(&event_tx) != 0 {
                            event_tx
                                .send((oracle.asset_pair_info.asset_pair, attestation).into())
                                .expect("usable channel");
                        }
                    }) else {
                        continue 'attestations;
                    };

                    let OracleError::MissingEventId(event_id) = error else {
                        error!(
                            "The oracle scheduler failed to attest an event at date {}: {}",
                            next_time, error
                        );
                        continue 'attestations;
                    };

                    // We clone the context and event_tx to give them to a new spawned task
                    // which retry to attest the event later.
                    let cloned_context = Context::clone(&oracle_context);
                    let cloned_event_tx = event_tx.clone();
                    let asset_pair = oracle.asset_pair_info.asset_pair;

                    tokio::spawn(async move {
                        match retry_attest(&cloned_context.oracles()[&asset_pair], event_id).await {
                            Some(attestation) if Sender::receiver_count(&cloned_event_tx) != 0 => {
                                cloned_event_tx
                                    .send((asset_pair, attestation).into())
                                    .expect("usable channel");
                            }
                            None => {
                                error!(
                                    "The pricefeed did not find data for the event {} at date {}, it has been retried for too long, skipping",
                                    event_id,
                                    next_time);
                            }
                            _ => {}
                        }
                    });
                }
            }
        }
        unreachable!("Cron schedule can be consumed only after 2100")
    };

    tokio::select! {
        e = announcement_task => {e},
        e = attestation_task => {e},
    }
}

/// Retry attesting a pricefeed event with exponential backoff.
///
/// Returns the attestation if found, otherwise None if retrying for too long.
async fn retry_attest(oracle: &Oracle, event_id: EventId) -> Option<Attestation> {
    let mut duration = Duration::from_secs(5);

    loop {
        info!(
            "The pricefeed did not find data for the event {}, it will retry in {} seconds",
            event_id,
            duration.as_secs()
        );

        sleep(duration).await;

        let maybe_attestation = oracle.try_attest_event(event_id).await;

        if let Ok(Some(attestation)) = maybe_attestation {
            break Some(attestation);
        }
        // exponential backoff
        duration = duration * 3 / 2;

        if duration.as_secs() > 60 {
            break None;
        }
    }
}
