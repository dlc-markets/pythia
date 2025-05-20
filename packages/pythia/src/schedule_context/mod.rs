use atomic_take::AtomicTake;
use chrono::Duration;
use cron::Schedule;
use scheduler::SchedulerContext;
use std::{borrow::Borrow, collections::HashMap};
use tokio::sync::{broadcast, oneshot};

use crate::{
    config::AssetPair,
    oracle::{error::OracleError, Oracle},
};

pub(super) mod api_context;
pub(super) mod error;
pub(super) mod scheduler;

use api_context::ApiContext;
use error::PythiaContextError;

/// The inner context of the oracle. It contains the configuration
/// settings for the oracle of each asset pair and the schedule.
/// It is used to create the `ApiContext` and `SchedulerContext`.
pub(super) struct OracleContextInner {
    oracles: HashMap<AssetPair, Oracle>,
    schedule: Schedule,
    error_sender: AtomicTake<oneshot::Sender<OracleError>>,
}

/// Initialize contexts for the API and scheduler that are internally bound together through a channel sending scheduled events to websocket context
///
/// Not all bounds on `Context` are explicit here. To run the API you actually need `Context` to be `Send` and `Unpin` as well.
/// You will often need to use `Arc` or `Box::leak` on the `OracleContextInner` to satisfy all the bounds with a reference.
pub(super) fn create_contexts<Context>(
    oracles: HashMap<AssetPair, Oracle>,
    schedule: Schedule,
    offset_duration: Duration,
    borrow_mod: impl FnOnce(OracleContextInner) -> Context,
) -> Result<(SchedulerContext<Context>, ApiContext<Context>), PythiaContextError>
where
    Context: OracleContext + Clone,
{
    let (error_sender, error_rx) = oneshot::channel::<OracleError>();

    let oracle_context = borrow_mod(OracleContextInner {
        oracles,
        schedule,
        error_sender: AtomicTake::new(error_sender),
    });

    // We set channel size to 2 for each oracle because it may happen that an announcement and attestation are sent into the channel
    // at the same time (if offset is a multiple of the attestation frequency schedule)
    let channel_size = 2 * oracle_context.oracles().len();
    // We do not need Receiver right now as we only use them to send events to websockets
    // so we use Sender::new to create a only half of the channel with the given size.
    let channel_sender = broadcast::Sender::new(channel_size);

    Ok((
        SchedulerContext::new(
            Context::clone(&oracle_context),
            offset_duration,
            channel_sender.clone(),
            error_rx,
        )?,
        ApiContext {
            oracle_context,
            offset_duration,
            channel_sender,
        },
    ))
}

pub(crate) trait OracleContext {
    fn oracles(&self) -> &HashMap<AssetPair, Oracle>;
    fn schedule(&self) -> &Schedule;
    fn send_error(&self, error: OracleError);
}

impl<T> OracleContext for T
where
    T: Borrow<OracleContextInner>,
{
    fn oracles(&self) -> &HashMap<AssetPair, Oracle> {
        &self.borrow().oracles
    }
    fn schedule(&self) -> &Schedule {
        &self.borrow().schedule
    }
    fn send_error(&self, error: OracleError) {
        if let Some(sender) = self.borrow().error_sender.take() {
            let _ = sender.send(error);
        }
    }
}

/// Box then leak the input.
pub(super) fn leaked<T>(context: T) -> &'static T {
    &*Box::leak(Box::new(context))
}
