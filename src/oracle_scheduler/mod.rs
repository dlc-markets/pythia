use crate::{common::OracleSchedulerConfig, oracle::Oracle, ws::EventNotification};

use chrono::Utc;
use log::info;

use std::sync::Arc;
use tokio::sync::broadcast::Sender;
use tokio::time::sleep;

mod error;
pub use error::OracleSchedulerError;
pub use error::Result;

extern crate hex;

pub async fn start_schedule(
    oracles: Box<[Arc<Oracle>]>,
    config: &OracleSchedulerConfig,
    event_tx: Sender<EventNotification>,
) -> Result<()> {
    let offset_duration = match config.announcement_offset.to_std() {
        Ok(duration) => duration,
        Err(_) => {
            return Err(OracleSchedulerError::InvalidAnnouncementTimeError(
                config.announcement_offset,
            ))
        }
    };

    // start event creation task
    info!("creating oracle events and schedules");

    let cloned_oracles = oracles.clone();
    let cloned_event_tx = event_tx.clone();
    let start_time = Utc::now();
    let attestation_scheduled_dates = config
        .schedule
        .after_owned(start_time.clone())
        .map(|date| date);
    let announcement_scheduled_dates = config
        .schedule
        .after_owned(start_time)
        .map(move |date| date - offset_duration);

    let announcement_thread = async move {
        for next_time in announcement_scheduled_dates {
            if let Ok(duration) = (next_time - Utc::now()).to_std() {
                info!("next announcement at {} in {:?}", &next_time, &duration);
                sleep(duration).await;
            };

            for oracle in cloned_oracles.iter() {
                let perhaps_announcement = oracle
                    .create_announcement(next_time + offset_duration)
                    .await;

                match perhaps_announcement {
                    Ok(announcement) => {
                        cloned_event_tx
                            .send((oracle.asset_pair_info.asset_pair, announcement).into())
                            .expect("usable channel");
                    }
                    Err(e) => return e,
                }
            }
        }
        // never be reached
        unreachable!()
    };

    let attestation_thread = async move {
        for next_time in attestation_scheduled_dates {
            if let Ok(duration) = (next_time - Utc::now()).to_std() {
                info!("next attestation at {} in {:?}", &next_time, &duration);
                sleep(duration).await;
            };

            for oracle in oracles.iter() {
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
                    Err(e) => return e,
                }
            }
        }
        // never be reached
        unreachable!()
    };

    let mut schedule_err = None;
    tokio::select! {
        e = announcement_thread => {schedule_err = Some(e)},
        e = attestation_thread => {schedule_err = Some(e)},
    };

    panic!(
        "scheduler stopped because of error {} \n The oracle is not announcing anymore",
        schedule_err.unwrap()
    );
}
