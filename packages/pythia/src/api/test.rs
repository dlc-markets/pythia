use actix_web::App;
use chrono::{Duration, Timelike as _, Utc};
use sqlx::PgPool;
use tokio::{sync::broadcast, task::LocalSet};

use crate::{
    api::v1_app_factory,
    data_models::{asset_pair::AssetPair, event_ids::EventId},
    schedule_context::{api_context::ApiContext, OracleContext as _},
};

use crate::test::schedule_context::{HandlerToMock, MockContext};

pub async fn run_in_local_set<F>(f: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    LocalSet::new().run_until(f).await
}

/// Populates the database with announcement events for testing
///
/// Creates a specified number of announcements with different maturity times
/// and stores their event IDs for later use in tests.
/// Sets up mocked pricefeed data before creating announcements.
pub async fn populate_test_db(
    mock_context: &mut MockContext,
    count: usize,
    context_handler: &HandlerToMock,
) -> Vec<EventId> {
    let now = Utc::now()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();

    // Pre-generate event IDs and set up mocked pricefeed data
    let mut event_ids = Vec::with_capacity(count);

    // Find the BTC/USD oracle
    let oracle = mock_context
        .oracles()
        .get(&AssetPair::BtcUsd)
        .expect("BTC/USD oracle must exist");

    // Create announcements at different times
    for i in 0..count {
        let maturity_time = now + Duration::hours(i as i64 + 1);

        let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, maturity_time);
        // Set up mocked pricefeed data for all the events we're about to create
        let _ = context_handler.send(vec![(event_id, Some(50000.0 + i as f64))]);

        // Create announcement
        let mut announcements = oracle
            .create_announcements_at_date(maturity_time)
            .await
            .expect("Failed to create announcement");

        event_ids.push(event_id);

        // Verify the event ID matches what we expected
        assert_eq!(announcements.pop().unwrap().oracle_event.event_id, event_id);
    }

    event_ids
}

/// Create a test server with the WebSocket endpoint and mocked pricefeed
///
/// This creates an Actix test server with our API context and WebSocket route
/// and sets up some basic mocked pricefeed data for testing
pub async fn get_test_server(pool: PgPool) -> (HandlerToMock, MockContext, actix_test::TestServer) {
    let channel_sender = broadcast::Sender::new(32);
    let (context_handler, context) = MockContext::new(pool).await;

    // Set up some basic mocked pricefeed data for testing
    let now = Utc::now()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    let event_id = EventId::spot_from_pair_and_timestamp(AssetPair::BtcUsd, now);
    let _ = context_handler.send(vec![(event_id, Some(50000.0))]);

    let oracle = context.oracles().get(&AssetPair::BtcUsd).unwrap();
    oracle.create_announcements_at_date(now).await.unwrap();

    let api_context = ApiContext {
        oracle_context: context.clone(),
        offset_duration: chrono::Duration::minutes(2),
        channel_sender,
    };

    (
        context_handler,
        context,
        actix_test::start(move || {
            let factory = v1_app_factory::<MockContext>(true);

            App::new().app_data(api_context.clone()).service(factory)
        }),
    )
}
