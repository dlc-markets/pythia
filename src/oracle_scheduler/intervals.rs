use core::future::Future;

use chrono::{NaiveTime, Utc};
use clokwerk::{AsyncJob, AsyncScheduler, Interval, Job};

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum MyInterval {
    /// The next multiple of `n` seconds since the start of the Unix epoch
    Seconds(u32),
    /// The next multiple of `n` minutes since the start of the day
    Minutes(u32),
    /// The next multiple of `n` hours since the start of the day
    Hours(u32),
    /// The next multiple of `n` days since the start of the start of the era
    Days(u32),
    /// The next multiple of `n` week since the start of the start of the era
    Weeks(u32),
    /// Every Monday
    Monday,
    /// Every Tuesday
    Tuesday,
    /// Every Wednesday
    Wednesday,
    /// Every Thursday
    Thursday,
    /// Every Friday
    Friday,
    /// Every Saturday
    Saturday,
    /// Every Sunday
    Sunday,
    /// Every weekday (Monday through Friday)
    Weekday,
}

#[derive(Debug)]
enum MyAdjustment {
    Intervals(Vec<MyInterval>),
    Time(NaiveTime),
}

#[derive(Debug)]
pub(crate) struct MyRunConfig {
    base: MyInterval,
    adjustment: Option<MyAdjustment>,
}

/// A RunConfig defines a schedule for a recurring event. It's composed of a base [`Interval`], and an additional adjustment.
/// The adjustment is either a single day offset (e.g. "at 3 AM") for use in conjunction with a base interval like "every three days", or "every Tuesday",
/// or it's a sequence of additional intervals, with the intended use of providing an additional offset for the scheduled task e.g.
/// "Every three hours, plus 30 minutes, plus 10 seconds".

pub(crate) fn run_with_config<'a, 'b, F, T>(
    scheduler: &'a mut AsyncScheduler<Utc>,
    config: &'b MyRunConfig,
    closure: F,
) -> &'a mut AsyncJob<Utc>
where
    F: 'static + FnMut() -> T + Send,
    T: 'static + Future<Output = ()> + Send,
{
    let mut job = scheduler.every(convert_interval(&config.base));
    if let Some(adjustement) = &config.adjustment {
        match adjustement {
            MyAdjustment::Intervals(intervals) => {
                for interval in intervals {
                    job = job.plus(convert_interval(&interval));
                }
                job
            }
            MyAdjustment::Time(t) => job.at_time(*t),
        }
    } else {
        job
    }
}

fn convert_interval(interval: &MyInterval) -> Interval {
    match interval {
        MyInterval::Seconds(t) => Interval::Seconds(*t),
        MyInterval::Minutes(t) => Interval::Minutes(*t),
        MyInterval::Hours(t) => Interval::Hours(*t),
        MyInterval::Days(t) => Interval::Days(*t),
        MyInterval::Weeks(t) => Interval::Weeks(*t),
        MyInterval::Monday => Interval::Monday,
        MyInterval::Tuesday => Interval::Tuesday,
        MyInterval::Wednesday => Interval::Wednesday,
        MyInterval::Thursday => Interval::Thursday,
        MyInterval::Friday => Interval::Friday,
        MyInterval::Saturday => Interval::Saturday,
        MyInterval::Sunday => Interval::Sunday,
        MyInterval::Weekday => Interval::Weekday,
    }
}
