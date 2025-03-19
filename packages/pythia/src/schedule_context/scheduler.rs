use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::{sync::Arc, time::Duration};
use tokio::{sync::broadcast::Sender, time::sleep};

use super::{error::PythiaContextError, OracleContextInner};
use crate::{api::EventNotification, error::PythiaError};

/// The API has shared ownership of the running oracles and schedule configuration file with the scheduler
/// Its context also include the channel receiver endpoint to broadcast announcements/attestations
pub(crate) struct SchedulerContext {
    oracle_context: Arc<OracleContextInner>,
    offset_duration: Duration,
    channel_sender: Sender<EventNotification>,
}

impl SchedulerContext {
    pub(super) fn new(
        oracle_context: Arc<OracleContextInner>,
        offset_duration: ChronoDuration,
        channel_sender: Sender<EventNotification>,
    ) -> Result<Self, PythiaContextError> {
        // This is to prevent an eventual UB produced in start_schedule by reaching "unreachable" marked code
        // The configured cron schedule may not produce a value although it is correctly parsed
        // Using "59 59 23 31 11 * 2100" as cron schedule in config file trigger this error in current cron crate version
        oracle_context.schedule.upcoming(Utc).next().ok_or(
            PythiaContextError::CronScheduleProduceNoValue(oracle_context.schedule.clone()),
        )?;

        let offset_duration = offset_duration.to_std()?;
        Ok(Self {
            oracle_context,
            offset_duration,
            channel_sender,
        })
    }
}

/// Start the scheduler of announcements and attestations using the context made with the api one.
/// It computes a date iterator from cron-like config and spawns a thread for each type of event.
/// At each iteration it sleeps if necessary without blocking until the next date produced by the iterator.
pub(crate) async fn start_schedule(context: SchedulerContext) -> Result<(), PythiaError> {
    let oracle_context = context.oracle_context;
    let cloned_oracle_context = Arc::clone(&oracle_context);
    let event_tx = context.channel_sender;

    // start event creation task
    info!("creating oracle events and schedules");

    let cloned_event_tx = event_tx.clone();
    let start_time = Utc::now();
    let attestation_scheduled_dates = oracle_context.schedule.after_owned(start_time);
    let announcement_scheduled_dates = oracle_context
        .schedule
        .after_owned(start_time)
        .map(move |date| date - context.offset_duration);
    let mut pending_maturations: Vec<DateTime<Utc>> = Vec::new();

    let announcement_thread = async move {
        for next_time in announcement_scheduled_dates {
            // We compute how much time we may have to sleep before continue
            // Converting into std Duration type fail here if we don't have to sleep
            let maybe_std_duration = (next_time - Utc::now()).to_std();
            if let Ok(duration) = maybe_std_duration {
                debug!(
                    "next announcement at {} in {:?} with maturity {}",
                    &next_time,
                    &duration,
                    next_time + context.offset_duration
                );

                if !pending_maturations.is_empty() {
                    debug!("pending maturations size: {:?}", pending_maturations.len());
                    for (_, oracle) in oracle_context.oracles.iter() {
                        let _ = oracle.create_many_announcements(&pending_maturations).await;
                    }
                    pending_maturations.clear();
                }

                sleep(duration).await;

                for (_, oracle) in oracle_context.oracles.iter() {
                    let perhaps_announcement = oracle
                        .create_announcement(next_time + context.offset_duration)
                        .await;

                    // To avoid flooding the websocket with announcements when starting we only broadcast the announcement if we had to sleep
                    cloned_event_tx
                        .send((oracle.asset_pair_info.asset_pair, perhaps_announcement?).into())
                        .expect("usable channel");
                }
            } else {
                // We accumulate announcements in a vector to avoid frequent individual database insertions.
                // When (next_time - Utc::now()) is negative, it means the event has already matured or is
                // imminent, so we continue accumulating announcements rather than inserting them immediately.
                // Once (next_time - Utc::now()) becomes non-negative (future events), we perform a bulk
                // insertion of all accumulated announcements into the PostgreSQL database with a single query
                // for better performance.
                pending_maturations.push(next_time + context.offset_duration);
            };
        }
        unreachable!("Cron schedule can be consumed only after 2100")
    };

    let attestation_thread = async move {
        for next_time in attestation_scheduled_dates {
            if let Ok(duration) = (next_time - Utc::now()).to_std() {
                debug!("next attestation at {} in {:?}", &next_time, &duration);
                sleep(duration).await;
            };

            for (_, oracle) in cloned_oracle_context.oracles.iter() {
                let event_id = oracle.asset_pair_info.asset_pair.to_string().to_lowercase()
                    + next_time.timestamp().to_string().as_str();

                let perhaps_attestation = oracle.try_attest_event(&event_id).await;

                match perhaps_attestation {
                    Ok(Some(attestation)) => {
                        event_tx
                            .send(
                                (
                                    oracle.asset_pair_info.asset_pair,
                                    attestation,
                                    event_id.into_boxed_str(),
                                )
                                    .into(),
                            )
                            .expect("usable channel");
                    }
                    Ok(None) => debug!("Already attested: {}", event_id),
                    Err(e) => error!("The oracle scheduler failed to attest: {}", &e.to_string()),
                }
            }
        }
        unreachable!("Cron schedule can be consumed only after 2100")
    };

    tokio::select! {
        e = announcement_thread => {e},
        e = attestation_thread => {e},
    }
}
