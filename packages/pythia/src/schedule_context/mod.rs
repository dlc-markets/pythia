use cron::Schedule;
use scheduler::SchedulerContext;
use std::collections::HashMap;
use tokio::sync::broadcast::Sender;

use crate::{
    config::{AssetPair, OracleSchedulerConfig},
    oracle::Oracle,
};

pub(super) mod api_context;
pub(super) mod error;
pub(super) mod scheduler;

use api_context::ApiContext;
use error::PythiaContextError;

#[derive(Clone, Copy)]
struct OracleContextInner<'a> {
    oracles: &'a HashMap<AssetPair, Oracle>,
    schedule: &'a Schedule,
}

/// Initialize contexts for the API and scheduler bind internally together through a channel sending scheduled event to websocket context
pub(super) fn create_contexts(
    oracles: &'static HashMap<AssetPair, Oracle>,
    scheduler_config: &'static OracleSchedulerConfig,
) -> Result<(SchedulerContext, ApiContext), PythiaContextError> {
    let oracle_context = OracleContextInner {
        oracles,
        schedule: &scheduler_config.schedule,
    };
    let offset_duration = scheduler_config.announcement_offset;
    // We set channel size to 2 for each oracle because it may happen that an announcement and attestation are sent into the channel
    // at the same time (if offset is a multiple of the attestation frequency schedule)
    let channel_size = 2 * oracle_context.oracles.len();
    // We do not need Receiver right now as we only use them to send events to websockets
    // so we use Sender::new to create a only half of the channel with the given size.
    let channel_sender = Sender::new(channel_size);
    Ok((
        SchedulerContext::new(oracle_context, offset_duration, channel_sender.clone())?,
        ApiContext {
            oracle_context,
            offset_duration,
            channel_sender,
        },
    ))
}
