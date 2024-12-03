use cron::Schedule;
use scheduler::SchedulerContext;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::broadcast::channel;

use crate::{
    api::EventNotification,
    config::{AssetPair, OracleSchedulerConfig},
    oracle::Oracle,
};

pub(super) mod api_context;
pub(super) mod error;
pub(super) mod scheduler;

use api_context::ApiContext;
use error::PythiaContextError;

pub(crate) struct OracleContextInner {
    pub(crate) oracles: HashMap<AssetPair, Oracle>,
    pub(crate) schedule: Schedule,
}

/// Initialize contexts for the API and scheduler bind internally together through a channel sending scheduled event to websocket context
pub(super) fn create_contexts(
    oracles: HashMap<AssetPair, Oracle>,
    scheduler_config: OracleSchedulerConfig,
) -> Result<(SchedulerContext, ApiContext), PythiaContextError> {
    let oracle_context = Arc::new(OracleContextInner {
        oracles,
        schedule: scheduler_config.schedule,
    });
    let offset_duration = scheduler_config.announcement_offset;
    // We set channel size to 2 for each oracle because it may happen that an announcement and attestation are sent into the channel
    // at the same time (if offset is a multiple of the attestation frequency schedule)
    let channel_size = 2 * oracle_context.oracles.len();
    let (channel_sender, channel_receiver) = channel::<EventNotification>(channel_size);
    Ok((
        SchedulerContext::new(oracle_context.clone(), offset_duration, channel_sender)?,
        ApiContext {
            oracle_context,
            offset_duration,
            channel_receiver,
        },
    ))
}
