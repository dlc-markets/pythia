use chrono::Duration;
use cron::Schedule;
use scheduler::SchedulerContext;
use std::{borrow::Borrow, collections::HashMap};
use tokio::sync::broadcast::Sender;

use crate::{config::AssetPair, oracle::Oracle};

pub(super) mod api_context;
pub(super) mod error;
pub(super) mod scheduler;

use api_context::ApiContext;
use error::PythiaContextError;

/// The inner context of the oracle. It contains the configuration
/// settings for the oracle of each asset pair and the schedule.
/// It is used to create the `ApiContext` and `SchedulerContext`.
#[derive(Clone)]
pub(super) struct OracleContextInner {
    pub(super) oracles: HashMap<AssetPair, Oracle>,
    pub(super) schedule: Schedule,
}

/// Initialize contexts for the API and scheduler that are internally bound together through a channel sending scheduled events to websocket context
///
/// Not all bounds on `Context` are explicit here. To run the API you actually need `Context` to be `Send` and `Unpin` as well.
/// You will often need to use `Arc` or `Box::leak` on the `OracleContextInner` to satisfy all the bounds with a reference.
pub(super) fn create_contexts<Context>(
    oracle_context: Context,
    offset_duration: Duration,
) -> Result<(SchedulerContext<Context>, ApiContext<Context>), PythiaContextError>
where
    Context: OracleContext + Clone + 'static,
{
    // We set channel size to 2 for each oracle because it may happen that an announcement and attestation are sent into the channel
    // at the same time (if offset is a multiple of the attestation frequency schedule)
    let channel_size = 2 * oracle_context.borrow().oracles.len();
    // We do not need Receiver right now as we only use them to send events to websockets
    // so we use Sender::new to create a only half of the channel with the given size.
    let channel_sender = Sender::new(channel_size);
    Ok((
        SchedulerContext::new(
            Context::clone(&oracle_context),
            offset_duration,
            channel_sender.clone(),
        )?,
        ApiContext {
            oracle_context,
            offset_duration,
            channel_sender,
        },
    ))
}

/// A trait for types that can immutably borrow an `OracleContextInner`.
pub(crate) trait OracleContext: Borrow<OracleContextInner> {}

impl<T: Borrow<OracleContextInner>> OracleContext for T {}
