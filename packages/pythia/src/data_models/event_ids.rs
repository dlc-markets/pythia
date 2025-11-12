use std::{error::Error, fmt, ops::Deref, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::Visitor};
use sqlx::{
    Database, Postgres,
    encode::IsNull,
    postgres::{PgHasArrayType, PgTypeInfo},
};

use crate::data_models::{ArrayString, asset_pair::AssetPair, error::ParsingError};

use sqlx::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventIdInfos {
    pub asset_pair: AssetPair,
    pub maturation: DateTime<Utc>,
    index_type: IndexType,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexType {
    Spot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(ArrayString<32>);

impl EventIdInfos {
    pub fn as_event_id(&self) -> EventId {
        match &self.index_type {
            IndexType::Spot => EventId(
                format_args!(
                    "{pair}{t}",
                    pair = self.asset_pair,
                    t = self.maturation.timestamp()
                )
                .try_into()
                .expect("We imposed a length of 17"),
            ),
        }
    }
}

impl TryFrom<EventId> for EventIdInfos {
    type Error = ParsingError;

    fn try_from(event_id: EventId) -> Result<Self, Self::Error> {
        match event_id.len() {
            17 => {
                let (asset_pair_str, maturation_str) = event_id.split_at(7);
                let asset_pair = asset_pair_str.parse().expect("invariant of event id type");
                let maturation = DateTime::from_timestamp(
                    maturation_str.parse().expect("invariant of event id type"),
                    0,
                )
                .expect("invariant of event id type");

                Ok(Self {
                    asset_pair,
                    maturation,
                    index_type: IndexType::Spot,
                })
            }
            _ => Err(ParsingError::InvalidLength {
                expected: 17,
                actual: event_id.len(),
            }),
        }
    }
}

impl EventIdInfos {
    pub fn spot_from_pair_and_timestamp(asset_pair: AssetPair, maturation: DateTime<Utc>) -> Self {
        EventIdInfos {
            asset_pair,
            maturation,
            index_type: IndexType::Spot,
        }
    }

    pub fn is_spot(&self) -> bool {
        self.index_type == IndexType::Spot
    }
}

impl sqlx::Type<Postgres> for EventId {
    fn type_info() -> PgTypeInfo {
        <&str as sqlx::Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str as sqlx::Type<Postgres>>::compatible(ty)
    }
}

impl Encode<'_, Postgres> for EventId {
    fn encode_by_ref(
        &self,
        buf: &mut <Postgres as Database>::ArgumentBuffer<'_>,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        <&str as Encode<'_, Postgres>>::encode_by_ref(&self.as_ref(), buf)
    }
}

impl<'r> Decode<'r, Postgres> for EventId {
    fn decode(
        value: <Postgres as Database>::ValueRef<'r>,
    ) -> Result<Self, Box<dyn Error + Sync + Send>> {
        Ok(<&str as Decode<'r, Postgres>>::decode(value)?.parse()?)
    }
}

impl PgHasArrayType for EventId {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as PgHasArrayType>::array_type_info()
    }
}

impl AsRef<str> for EventId {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

impl fmt::Display for EventIdInfos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        EventId::fmt(&self.as_event_id(), f)
    }
}

impl PartialOrd for EventId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.as_ref().cmp(other.as_ref()))
    }
}

impl Ord for EventId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl Deref for EventId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl FromStr for EventId {
    type Err = ParsingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.len() {
            17 => {
                let (asset_pair_str, maturation_str) =
                    value
                        .split_at_checked(7)
                        .ok_or_else(|| match value.chars().nth(7) {
                            Some(seventh_char) => ParsingError::InvalidAssetPair {
                                expected: "char boundary at 7th byte",
                                actual: format!("the 7th char is {seventh_char}"),
                            },
                            None => ParsingError::InvalidLength {
                                expected: 17,
                                actual: value.len(),
                            },
                        })?;

                let asset_pair = asset_pair_str.parse::<AssetPair>()?;
                let maturation = DateTime::from_timestamp(maturation_str.parse()?, 0)
                    .expect("check on length avoid overflow");

                Ok(EventId(
                    format_args!("{pair}{t}", pair = asset_pair, t = maturation.timestamp())
                        .try_into()
                        .expect("We imposed a length of 17, less than 32"),
                ))
            }
            l => Err(ParsingError::InvalidLength {
                expected: 17,
                actual: l,
            }),
        }
    }
}

impl Serialize for EventId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self)
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EventIdVisitor;

        impl<'de> Visitor<'de> for EventIdVisitor {
            type Value = EventId;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string in the format [asset_pair][unix_timestamp]")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse().map_err(|_| E::invalid_length(v.len(), &"17"))
            }
        }
        deserializer.deserialize_str(EventIdVisitor)
    }
}
