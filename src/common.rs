use crate::pricefeeds::ImplementedPriceFeed;
use chrono::Duration;
use cron::Schedule;
use dlc_messages::oracle_msgs::EventDescriptor;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use std::fmt::{self, Debug, Display, Formatter};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetPair {
    Btcusd,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub struct AssetPairInfo {
    pub pricefeed: ImplementedPriceFeed,
    pub asset_pair: AssetPair,
    pub event_descriptor: EventDescriptor,
}

impl Display for AssetPair {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

mod standard_duration {
    use chrono::Duration;
    use serde::{
        de::{self, Visitor},
        Deserializer, Serializer,
    };
    use std::fmt;

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(
            &humantime::format_duration(std::time::Duration::from_secs(
                value.num_seconds().try_into().unwrap(),
            ))
            .to_string(),
        )
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DurationVisitor;

        impl<'de> Visitor<'de> for DurationVisitor {
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
                        .unwrap(),
                ))
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OracleSchedulerConfig {
    #[serde_as(as = "DisplayFromStr")]
    pub schedule: Schedule,
    #[serde(with = "standard_duration")]
    pub announcement_offset: Duration,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub struct ConfigurationFile {
    pub asset_pair_infos: Vec<AssetPairInfo>,
    pub oracle_scheduler_config: OracleSchedulerConfig,
}

#[serde_as]
#[derive(Serialize)]
pub struct ConfigResponse {
    pricefeed: ImplementedPriceFeed,
    #[serde(with = "standard_duration")]
    announcement_offset: Duration,
    #[serde_as(as = "DisplayFromStr")]
    pub schedule: Schedule,
}

impl From<(ImplementedPriceFeed, OracleSchedulerConfig)> for ConfigResponse {
    fn from(
        (pricefeed, oracle_scheduler_config): (ImplementedPriceFeed, OracleSchedulerConfig),
    ) -> Self {
        ConfigResponse {
            pricefeed,
            announcement_offset: oracle_scheduler_config.announcement_offset,
            schedule: oracle_scheduler_config.schedule,
        }
    }
}
