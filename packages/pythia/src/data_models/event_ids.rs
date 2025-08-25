use std::{error::Error, fmt, ops::Deref, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{de::Visitor, Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    postgres::{PgHasArrayType, PgTypeInfo},
    Database, Postgres,
};

use crate::data_models::{asset_pair::AssetPair, ArrayString};

use sqlx::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventId {
    Spot(ArrayString<17>),
}

impl EventId {
    pub fn spot_from_pair_and_timestamp(pair: AssetPair, timestamp: DateTime<Utc>) -> Self {
        Self::Spot(
            format_args!("{pair}{t}", t = timestamp.timestamp())
                .try_into()
                .expect("We imposed a length of 17"),
        )
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

impl PgHasArrayType for EventId {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as PgHasArrayType>::array_type_info()
    }
}

impl AsRef<str> for EventId {
    fn as_ref(&self) -> &str {
        match self {
            Self::Spot(s) => s.as_ref(),
        }
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_ref())
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
    type Err = usize;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.len() {
            17 => Ok(EventId::Spot(
                value[..17].parse().expect("Length already checked"),
            )),
            l => Err(l),
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
                Ok(EventId::Spot(
                    v.parse().map_err(|_| E::invalid_length(v.len(), &"17"))?,
                ))
            }
        }
        deserializer.deserialize_str(EventIdVisitor)
    }
}
