use chrono::OutOfRangeError;
use cron::Schedule;
use displaydoc::Display;
use thiserror::Error;

#[derive(Debug, Display, Error)]
pub(crate) enum PythiaContextError {
    /// the Cron schedule {0} does not produce any date value
    CronScheduleProduceNoValue(Schedule),

    /// duration value is not valid: {0}
    InvalidDuration(#[from] OutOfRangeError),
}
