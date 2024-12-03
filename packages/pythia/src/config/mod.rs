use crate::pricefeeds::ImplementedPriceFeed;
use chrono::Duration;
use cron::Schedule;
use dlc_messages::oracle_msgs::DigitDecompositionEventDescriptor;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use std::fmt::{self, Debug, Display, Formatter};
use strum::EnumIter;

pub(super) mod cli;
mod env;
pub(super) mod error;

#[derive(Copy, Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, EnumIter)]
#[serde(rename_all = "snake_case")]
pub(super) enum AssetPair {
    #[default]
    BtcUsd,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AssetPairInfo {
    pub(super) pricefeed: ImplementedPriceFeed,
    pub(super) asset_pair: AssetPair,
    #[serde(default = "default_digit_event")]
    pub(super) event_descriptor: DigitDecompositionEventDescriptor,
}

fn default_digit_event() -> DigitDecompositionEventDescriptor {
    DigitDecompositionEventDescriptor {
        base: 2,
        is_signed: false,
        unit: "usd".to_owned(),
        precision: 0,
        nb_digits: 20,
    }
}

impl Display for AssetPair {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            AssetPair::BtcUsd => write!(f, "btc_usd"),
        }
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
                        .unwrap(),
                ))
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct OracleSchedulerConfig {
    #[serde_as(as = "DisplayFromStr")]
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
                self.announcement_offset.num_seconds().try_into().unwrap()
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

#[serde_as]
#[derive(Serialize)]
pub(super) struct ConfigResponse {
    pub(super) pricefeed: ImplementedPriceFeed,
    #[serde(with = "standard_duration")]
    pub(super) announcement_offset: Duration,
    #[serde_as(as = "DisplayFromStr")]
    pub(super) schedule: Schedule,
}
