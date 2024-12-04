use std::sync::Arc;

use actix_utils::future::{err, ok, Ready};
use actix_web::{
    error::{Error, ErrorInternalServerError},
    FromRequest,
};
use chrono::Duration;
use cron::Schedule;
use tokio::sync::broadcast::Receiver;

use crate::{api::EventNotification, oracle::Oracle};

use super::{AssetPair, OracleContextInner};

/// The API has shared ownership of the running oracles and schedule configuration file with the scheduler.
/// This context also includes the channel receiver endpoint to broadcast announcements/attestations
pub(crate) struct ApiContext {
    pub(super) oracle_context: Arc<OracleContextInner>,
    pub(crate) offset_duration: Duration,
    pub(crate) channel_receiver: Receiver<EventNotification>,
}

impl ApiContext {
    /// Get iterator over AssetPairs
    pub(crate) fn asset_pairs(&self) -> impl Iterator<Item = &AssetPair> {
        self.oracle_context.oracles.keys()
    }

    /// Get the oracle for the given asset pair
    pub(crate) fn get_oracle(&self, asset_pair: &AssetPair) -> Option<&Oracle> {
        self.oracle_context.oracles.get(asset_pair)
    }

    /// Get the schedule
    pub(crate) fn schedule(&self) -> &Schedule {
        &self.oracle_context.schedule
    }
}

impl Clone for ApiContext {
    fn clone(&self) -> Self {
        Self {
            oracle_context: Arc::clone(&self.oracle_context),
            offset_duration: self.offset_duration,
            channel_receiver: self.channel_receiver.resubscribe(),
        }
    }
}

/// The ApiContext must be made available when the app is responding to request
impl FromRequest for ApiContext {
    type Error = Error;

    type Future = Ready<Result<Self, Error>>;

    fn from_request(
        req: &actix_web::HttpRequest,
        _payload: &mut actix_web::dev::Payload,
    ) -> Self::Future {
        if let Some(st) = req.app_data::<ApiContext>() {
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
