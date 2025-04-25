use actix_utils::future::{err, ok, Ready};
use actix_web::{
    error::{Error, ErrorInternalServerError},
    FromRequest,
};
use chrono::Duration;
use cron::Schedule;
use tokio::sync::broadcast::Sender;

use crate::{api::EventNotification, oracle::Oracle};

use super::{AssetPair, OracleContext};

/// The API holds a reference to the running oracles and schedule configuration file shared with the scheduler.
/// This context also includes the channel receiver endpoint for broadcasting announcements/attestations,
/// and the channel sender for sending events to websockets (only used when forcing attestations in debug mode).
pub(crate) struct ApiContext<Context> {
    pub(super) oracle_context: Context,
    pub(crate) offset_duration: Duration,
    pub(crate) channel_sender: Sender<EventNotification>,
}

impl<Context: OracleContext + Clone> Clone for ApiContext<Context> {
    fn clone(&self) -> Self {
        Self {
            oracle_context: Context::clone(&self.oracle_context),
            offset_duration: self.offset_duration,
            channel_sender: self.channel_sender.clone(),
        }
    }
}

impl<Context: OracleContext> ApiContext<Context> {
    /// Get iterator over AssetPairs
    pub(crate) fn asset_pairs(&self) -> impl Iterator<Item = &AssetPair> {
        self.oracle_context.borrow().oracles.keys()
    }

    /// Get the oracle for the given asset pair
    pub(crate) fn get_oracle(&self, asset_pair: &AssetPair) -> Option<&Oracle> {
        self.oracle_context.borrow().oracles.get(asset_pair)
    }

    /// Get the schedule
    pub(crate) fn schedule(&self) -> &Schedule {
        &self.oracle_context.borrow().schedule
    }
}

/// The ApiContext must be made available when the app is responding to request
impl<Context: OracleContext + Clone + 'static> FromRequest for ApiContext<Context> {
    type Error = Error;

    type Future = Ready<Result<Self, Error>>;

    fn from_request(
        req: &actix_web::HttpRequest,
        _payload: &mut actix_web::dev::Payload,
    ) -> Self::Future {
        if let Some(st) = req.app_data::<ApiContext<Context>>() {
            ok(st.clone())
        } else {
            log::info!(
                "Failed to construct App-level Data extractor. \
                 Request path: {:?}",
                req.path()
            );
            err(ErrorInternalServerError(
                "App data is not configured, to configure use App::data()",
            ))
        }
    }
}
