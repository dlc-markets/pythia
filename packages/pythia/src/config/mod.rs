use crate::{
    data_models::{asset_pair::AssetPair, oracle_msgs::DigitDecompositionEventDesc},
    pricefeeds::ImplementedPriceFeed,
};
use chrono::Duration;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Debug, Display, Formatter};

pub(super) mod cli;
mod env;
pub(super) mod error;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AssetPairInfo {
    pub(super) pricefeed: ImplementedPriceFeed,
    pub(super) asset_pair: AssetPair,
    #[serde(default)]
    pub(super) event_descriptor: DigitDecompositionEventDesc,
}

mod standard_duration {
    use chrono::Duration;
    use serde::{
        de::{self, Visitor},
        ser::Error as _,
        Deserializer, Serializer,
    };
    use std::fmt;

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(
            &humantime::format_duration(std::time::Duration::from_secs(
                value.num_seconds().try_into().map_err(S::Error::custom)?,
            ))
            .to_string(),
        )
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DurationVisitor;

        impl Visitor<'_> for DurationVisitor {
            type Value = Duration;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("string that parses to time::Duration")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Duration::nanoseconds(
                    humantime::parse_duration(v)
                        .map_err(E::custom)?
                        .as_nanos()
                        .try_into()
                        .map_err(E::custom)?,
                ))
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct OracleSchedulerConfig {
    pub(super) schedule: Schedule,
    #[serde(with = "standard_duration")]
    pub(super) announcement_offset: Duration,
}

impl Display for OracleSchedulerConfig {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "OracleSchedulerConfig {{\n\tcron_schedule: {},\n\tannouncement_offset: {}\n}}",
            self.schedule,
            humantime::format_duration(std::time::Duration::from_secs(
                self.announcement_offset
                    .num_seconds()
                    .try_into()
                    .unwrap_or(0)
            ))
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigurationFile {
    pairs: Vec<AssetPairInfo>,
    #[serde(flatten)]
    oracle_scheduler_config: OracleSchedulerConfig,
}

#[derive(Serialize)]
pub(super) struct ConfigResponse<'a> {
    pub(super) pricefeed: &'a str,
    #[serde(with = "standard_duration")]
    pub(super) announcement_offset: Duration,
    pub(super) schedule: &'a Schedule,
}
