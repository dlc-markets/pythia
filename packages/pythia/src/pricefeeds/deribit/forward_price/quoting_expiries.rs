use std::sync::LazyLock;

use chrono::{DateTime, TimeDelta, Timelike, Utc};
use cron::Schedule;

// A frequency class of deribit expiry and the number of expiries
// to expect in this frequency class quoting continuously.
// A new expiry starts quoting 24 hours before another expiry
// of the same frequency class expires.
struct ForwardConfig {
    expiry_schedule: Schedule,
    nb_expiries: usize,
}

// Deribit quotes 4 type of expiries: 3 daily, 3 weekly, 3 monthly and 4 quarterly
static DERIBIT_QUOTING_FORWARD_CONFIG: LazyLock<[ForwardConfig; 4]> = LazyLock::new(|| {
    let daily_schedule = "0 0 8 * * * *".parse().expect("our valid schedule");
    let weekly_schedule = "0 0 8 * * FRIDAY *".parse().expect("our valid schedule");
    let weekly_last_schedule = "0 0 8 * * FRIDAYL *".parse().expect("our valid schedule");
    let quarterly_schedule = "0 0 8 * 3,6,9,12 FRIDAYL *"
        .parse()
        .expect("our valid schedule");

    [
        ForwardConfig {
            expiry_schedule: daily_schedule,
            // We expect 3 expiries to be quoted continuously for daily expiries.
            // But as another expiry is added 24 hours before the next expiry,
            // we must set nb_expiries to 2 to get 3 daily expiries continuously.
            nb_expiries: 2,
        },
        ForwardConfig {
            expiry_schedule: weekly_schedule,
            nb_expiries: 3,
        },
        ForwardConfig {
            expiry_schedule: weekly_last_schedule,
            nb_expiries: 3,
        },
        ForwardConfig {
            expiry_schedule: quarterly_schedule,
            nb_expiries: 4,
        },
    ]
});

/// Iterator over all expiries that are quoting at the given date. May have duplicates and need sorting.
pub(super) fn get_all_quoting_forward_at_date(
    date: DateTime<Utc>,
) -> impl Iterator<Item = DateTime<Utc>> {
    DERIBIT_QUOTING_FORWARD_CONFIG.iter().flat_map(
        move |&ForwardConfig {
                  ref expiry_schedule,
                  nb_expiries,
              }| {
            let mut expiries_iter = expiry_schedule.after(&date).peekable();

            let next_expiry = expiries_iter
                .peek()
                .copied()
                .expect("Our schedules always have a next date");

            // If next expiry is less than 24 hours away, then we must consider one more expiry
            // than specified by nb_expiries.
            let quoting_expiries =
                nb_expiries + (next_expiry - date <= TimeDelta::days(1)) as usize;

            expiries_iter.take(quoting_expiries)
        },
    )
}

/// Check if the given date is an expiry date.
pub(super) fn is_an_expiry_date(date: DateTime<Utc>) -> bool {
    // All expiries are every day at 8:00:00 UTC.
    date.hour() == 8 && date.minute() == 0 && date.second() == 0
}
