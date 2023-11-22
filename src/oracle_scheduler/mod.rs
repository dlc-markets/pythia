use crate::{common::OracleSchedulerConfig, oracle::Oracle};
use chrono::Utc;
use clokwerk::{AsyncScheduler, Interval};
use log::info;
use queues::{queue, IsQueue, Queue};
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::{
    sync::{mpsc, Mutex},
    time::sleep,
};

mod error;
pub use error::OracleSchedulerError;
pub use error::Result;

extern crate hex;

const SCHEDULER_SLEEP_TIME: std::time::Duration = std::time::Duration::from_millis(100);

struct OracleScheduler {
    oracle: Arc<Oracle>,
    config: OracleSchedulerConfig,
    event_queue: Queue<String>,
    next_announcement: OffsetDateTime,
    next_attestation: OffsetDateTime,
}

impl OracleScheduler {
    async fn create_scheduler_event(&mut self) -> Result<()> {
        let announcement_offset = self.config.announcement_offset;
        let announcement = self
            .oracle
            .create_announcement(self.next_announcement + announcement_offset)
            .await?;
        self.event_queue
            .add(announcement.oracle_event.event_id)
            .unwrap();
        self.next_announcement += self.config.frequency;
        Ok(())
    }

    async fn attest(&mut self) -> Result<()> {
        let event_id = self
            .event_queue
            .remove()
            .expect("queue should never be empty");

        let attestation = self.oracle.try_attest_event(event_id).await?;
        info!(
            "attesting with maturation {} and attestation {:#?}",
            self.next_attestation, attestation
        );
        self.next_attestation += self.config.frequency;
        Ok(())
    }
}

pub fn init(oracle: Arc<Oracle>, config: OracleSchedulerConfig) -> Result<()> {
    if !config.announcement_offset.is_positive() {
        return Err(OracleSchedulerError::InvalidAnnouncementTimeError(
            config.announcement_offset,
        ));
    }
    // start event creation task
    info!("creating oracle events and schedules");
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel();
        if let Err(err) = create_events(oracle, config, tx).await {
            panic!("oracle scheduler create_events error: {}", err);
        }
        if let Some(err) = rx.recv().await {
            panic!("oracle scheduler error: {}", err);
        }
        // never be reached
        unreachable!()
    });
    Ok(())
}

async fn create_events(
    oracle: Arc<Oracle>,
    config: OracleSchedulerConfig,
    error_transmitter: mpsc::UnboundedSender<OracleSchedulerError>,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let mut next_attestation = now
        .replace_second(0)
        .expect("0 seconde is valid")
        .replace_millisecond(0)
        .expect("Millisecond can be 0");
    if next_attestation <= now {
        next_attestation += config.frequency;
    }
    let mut next_announcement = next_attestation - config.announcement_offset;
    let mut event_queue = queue![];
    // create all events that should have already been made
    info!("creating events that should have already been made");
    while next_announcement <= now {
        let next_attestation = next_announcement + config.announcement_offset;
        match oracle
            .oracle_state("btcusd".to_string() + &next_attestation.unix_timestamp().to_string())
            .await
            .unwrap()
        {
            None => {
                let announcement = oracle.create_announcement(next_attestation).await?;
                event_queue.add(announcement.oracle_event.event_id).unwrap();
            }
            Some(val) => {
                info!(
                    "existing oracle event found in db with maturation {}, skipping creation",
                    next_attestation
                );
                event_queue.add(val.0.oracle_event.event_id).unwrap();
            }
        };
        next_announcement += config.frequency;
    }
    info!(
        "created new oracle scheduler with\n\tfrequency of {}\n\tnext announcement at {}\n\tnext attestation at {}",
        config.frequency,
        next_announcement,
        next_attestation
    );
    let oracle_scheduler = Arc::new(Mutex::new(OracleScheduler {
        oracle,
        config,
        event_queue,
        next_announcement,
        next_attestation,
    }));

    let mut scheduler = AsyncScheduler::with_tz(Utc);
    // schedule announcements
    let error_transmitter_clone = error_transmitter.clone();
    let oracle_scheduler_clone = oracle_scheduler.clone();
    let interval = Interval::Seconds(config.frequency.whole_seconds().try_into().unwrap());
    info!("starting announcement scheduler");
    scheduler.every(interval).run(move || {
        let oracle_scheduler_clone = oracle_scheduler_clone.clone();
        let error_transmitter_clone = error_transmitter_clone.clone();
        async move {
            if let Err(err) = oracle_scheduler_clone
                .lock()
                .await
                .create_scheduler_event()
                .await
            {
                info!("error from announcement scheduler");
                error_transmitter_clone.send(err).unwrap();
            }
        }
    });
    // schedule attestations
    info!("starting attestation scheduler");
    scheduler.every(interval).run(move || {
        let oracle_scheduler_clone = oracle_scheduler.clone();
        let error_transmitter_clone = error_transmitter.clone();
        async move {
            if let Err(err) = oracle_scheduler_clone.lock().await.attest().await {
                info!("error from attestation scheduler");
                error_transmitter_clone.send(err).unwrap();
            }
        }
    });
    // busy checking scheduler
    info!("starting busy checking");
    tokio::spawn(async move {
        loop {
            scheduler.run_pending().await;
            sleep(SCHEDULER_SLEEP_TIME).await;
        }
    });
    Ok(())
}
