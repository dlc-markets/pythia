use std::collections::HashMap;

use crate::config::AssetPair;
use crate::error;
use crate::{api::EventNotification, config::OracleSchedulerConfig, oracle::Oracle};

use chrono::Utc;
use log::info;

use tokio::sync::broadcast::Sender;
use tokio::time::sleep;

extern crate hex;

pub async fn start_schedule<'a>(
    oracles: &HashMap<AssetPair, Oracle<'a>>,
    config: &OracleSchedulerConfig,
    event_tx: Sender<EventNotification>,
) -> Result<(), error::PythiaError> {
    let offset_duration = match config.announcement_offset.to_std() {
        Ok(duration) => duration,
        Err(_) => {
            panic!(
                "Announcement offset {} is not a valid duration !",
                config.announcement_offset
            )
        }
    };

    // start event creation task
    info!("creating oracle events and schedules");

    let cloned_event_tx = event_tx.clone();
    let start_time = Utc::now();
    let attestation_scheduled_dates = config.schedule.after_owned(start_time);
    let announcement_scheduled_dates = config
        .schedule
        .after_owned(start_time)
        .map(move |date| date - offset_duration);

    let announcement_thread = async move {
        for next_time in announcement_scheduled_dates {
            // We compute how much time we may have to sleep before continue
            // Converting into std Duration type fail here if we don't have to sleep
            let maybe_std_duration = (next_time - Utc::now()).to_std();
            if let Ok(duration) = maybe_std_duration {
                info!("next announcement at {} in {:?}", &next_time, &duration);
                sleep(duration).await;
            };

            for (_, oracle) in oracles.iter() {
                let perhaps_announcement = oracle
                    .create_announcement(next_time + offset_duration)
                    .await;

                match perhaps_announcement {
                    Ok(announcement) => {
                        // To avoid flooding the websocket with announcements when starting we only broadcast the announcement if we had to sleep
                        if maybe_std_duration.is_ok() {
                            cloned_event_tx
                                .send((oracle.asset_pair_info.asset_pair, announcement).into())
                                .expect("usable channel");
                        }
                    }
                    Err(e) => return e,
                }
            }
        }
        unreachable!("Cron schedule can be consumed only after 2100")
    };

    let attestation_thread = async move {
        for next_time in attestation_scheduled_dates {
            if let Ok(duration) = (next_time - Utc::now()).to_std() {
                info!("next attestation at {} in {:?}", &next_time, &duration);
                sleep(duration).await;
            };

            for (_, oracle) in oracles.iter() {
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
        unreachable!("Cron schedule can be consumed only after 2100")
    };

    let schedule_err = tokio::select! {
        e = announcement_thread => {e},
        e = attestation_thread => {e},
    };

    Err(schedule_err.into())
}
