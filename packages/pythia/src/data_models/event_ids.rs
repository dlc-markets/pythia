use std::{error::Error, fmt, ops::Deref, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::Visitor};
use sqlx::{
    Database, Postgres,
    encode::IsNull,
    postgres::{PgHasArrayType, PgTypeInfo},
};

use crate::data_models::{
    ArrayString,
    asset_pair::AssetPair,
    error::ParsingError,
    expiries::{EXPIRY_LENGTH, Expiry},
};

use sqlx::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventIdInfos {
    pub asset_pair: AssetPair,
    pub maturation: DateTime<Utc>,
    index_type: IndexType,
}

const SPOT_LENGTH: usize = 17;
const FORWARD_LENGTH: usize = 26;
const DELIVERY_LENGTH: usize = 16;
const EXPIRY_OFFSET: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexType {
    Spot,
    Forward(Expiry),
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
            IndexType::Forward(expiry) if DateTime::<Utc>::from(*expiry) == self.maturation => {
                EventId(
                    format_args!("{pair}_{expiry}d", pair = self.asset_pair, expiry = expiry,)
                        .try_into()
                        .expect("We imposed a length of 17"),
                )
            }
            IndexType::Forward(expiry) => EventId(
                format_args!(
                    "{pair}_{expiry}f{t}",
                    pair = self.asset_pair,
                    expiry = expiry,
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
        event_id.parse::<EventIdInfos>()
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

    pub fn forward_of_expiry_with_pair_at_timestamp(
        pair: AssetPair,
        expiry: Expiry,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            asset_pair: pair,
            maturation: timestamp,
            index_type: IndexType::Forward(expiry),
        }
    }

    pub fn delivery_of_expiry_with_pair(pair: AssetPair, expiry: Expiry) -> Self {
        Self {
            asset_pair: pair,
            maturation: expiry.into(),
            index_type: IndexType::Forward(expiry),
        }
    }

    pub fn as_expiry(&self) -> Option<Expiry> {
        match &self.index_type {
            IndexType::Forward(expiry) => Some(*expiry),
            _ => None,
        }
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
        Some(self.cmp(other))
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

impl TryFrom<EventId> for Expiry {
    type Error = ParsingError;
    fn try_from(value: EventId) -> Result<Self, Self::Error> {
        match EventIdInfos::try_from(value)?.index_type {
            IndexType::Forward(expiry) => Ok(expiry),
            _ => Err(ParsingError::NoExpiryInEventId),
        }
    }
}

impl FromStr for EventIdInfos {
    type Err = ParsingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.len() {
            SPOT_LENGTH | FORWARD_LENGTH | DELIVERY_LENGTH => Ok(()),
            _ => Err(ParsingError::InvalidLength {
                expected: SPOT_LENGTH,
                actual: value.len(),
            }),
        }?;

        const ASSET_PAIR_LENGTH: usize = 7;
        let (asset_pair_str, trail) =
            value.split_at_checked(ASSET_PAIR_LENGTH).ok_or_else(|| {
                match value.chars().nth(ASSET_PAIR_LENGTH) {
                    Some(seventh_char) => ParsingError::InvalidChar {
                        expected: "char boundary at byte 7",
                        actual: format!("{seventh_char} at byte 7"),
                    },
                    None => ParsingError::InvalidLength {
                        expected: value.chars().count(),
                        actual: value.len(),
                    },
                }
            })?;

        let asset_pair = asset_pair_str.parse::<AssetPair>()?;

        match trail.len() {
            l if l == SPOT_LENGTH - ASSET_PAIR_LENGTH => {
                let maturation = DateTime::from_timestamp(trail.parse()?, 0)
                    .expect("check on length avoid overflow");

                Ok(EventIdInfos {
                    asset_pair,
                    maturation,
                    index_type: IndexType::Spot,
                })
            }
            l if l == FORWARD_LENGTH - ASSET_PAIR_LENGTH
                || l == DELIVERY_LENGTH - ASSET_PAIR_LENGTH =>
            {
                let (expiry, trail) = trail[EXPIRY_OFFSET - EXPIRY_LENGTH..]
                    .split_at_checked(EXPIRY_LENGTH)
                    .ok_or(ParsingError::NoExpiryInEventId)?;
                let expiry = expiry.parse::<Expiry>()?;

                match trail.split_at_checked(1) {
                    Some(("d", "")) => Ok(EventIdInfos {
                        asset_pair,
                        maturation: expiry.into(),
                        index_type: IndexType::Forward(expiry),
                    }),
                    Some(("f", start_time_str)) => {
                        let start_time = DateTime::from_timestamp(start_time_str.parse()?, 0)
                            .expect("check on length avoid overflow");
                        Ok(EventIdInfos {
                            asset_pair,
                            maturation: start_time,
                            index_type: IndexType::Forward(expiry),
                        })
                    }
                    _ => Err(ParsingError::NoExpiryInEventId),
                }
            }
            _ => unreachable!(
                "Only 3 values of length are possible from here for an event id: 17, 16, 26"
            ),
        }
    }
}

impl FromStr for EventId {
    type Err = ParsingError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        EventIdInfos::from_str(value).map(|infos| infos.as_event_id())
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
                formatter.write_str(
                    "a string in the format [asset_pair](_[expiry][f|d])?[unix_timestamp]?",
                )
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

#[cfg(test)]
mod tests_event_id {
    use super::*;

    #[test]
    fn test_event_id_parse() {
        let event_id_f1 = "BTC_USD_01JUL25f1717336000"
            .parse::<EventIdInfos>()
            .unwrap();
        let event_id_f2 = "BTC_USD_02JUL25f1717336000"
            .parse::<EventIdInfos>()
            .unwrap();

        assert!(matches!(event_id_f1.index_type, IndexType::Forward(_)));
        assert!(matches!(event_id_f2.index_type, IndexType::Forward(_)));

        let event_id_d1 = "BTC_USD_01JUL25d".parse::<EventIdInfos>().unwrap();
        let event_id_d2 = "BTC_USD_02JUL25d".parse::<EventIdInfos>().unwrap();

        assert!(matches!(event_id_d1.index_type, IndexType::Forward(_)));
        assert!(matches!(event_id_d2.index_type, IndexType::Forward(_)));

        let event_id_s1 = "BTC_USD1717336000".parse::<EventIdInfos>().unwrap();
        let event_id_s2 = "BTC_USD1717336000".parse::<EventIdInfos>().unwrap();

        assert!(matches!(event_id_s1.index_type, IndexType::Spot));
        assert!(matches!(event_id_s2.index_type, IndexType::Spot));
    }

    #[test]
    fn test_event_id_try_from_expiry() {
        let event_id_f1 = "BTC_USD_01JUL25f1717336000".parse::<EventId>().unwrap();
        let expiry = Expiry::try_from(event_id_f1).unwrap();
        assert_eq!(expiry, "01JUL25".parse::<Expiry>().unwrap());

        let event_id_d1 = "BTC_USD_01JUL25d".parse::<EventId>().unwrap();
        let expiry = Expiry::try_from(event_id_d1).unwrap();
        assert_eq!(expiry, "01JUL25".parse::<Expiry>().unwrap());

        let event_id_s1 = "BTC_USD1717336000".parse::<EventId>().unwrap();
        assert!(Expiry::try_from(event_id_s1).is_err());
    }

    #[test]
    fn test_event_id_serde() {
        let event_id_f1 = "BTC_USD_01JUL25f1717336000".parse::<EventId>().unwrap();
        let serde_event_id = serde_json::to_string(&event_id_f1).unwrap();
        assert_eq!(serde_event_id, "\"BTC_USD_01JUL25f1717336000\"");

        let de_ser_event_id: EventId = serde_json::from_str(&serde_event_id).unwrap();
        assert_eq!(event_id_f1, de_ser_event_id);

        let event_id_d1 = "BTC_USD_01JUL25d".parse::<EventId>().unwrap();
        let serde_event_id = serde_json::to_string(&event_id_d1).unwrap();
        assert_eq!(serde_event_id, "\"BTC_USD_01JUL25d\"");

        let de_ser_event_id: EventId = serde_json::from_str(&serde_event_id).unwrap();
        assert_eq!(event_id_d1, de_ser_event_id);
    }

    #[test]
    fn test_event_id_parse_failures() {
        assert!(matches!(
            "BTC_USD_01JUL25f17173360005".parse::<EventId>(),
            Err(ParsingError::InvalidLength {
                expected: SPOT_LENGTH,
                actual: 27,
            })
        ));

        assert!(matches!(
            "BTC_USD_BONJOURf1717336000".parse::<EventId>(),
            Err(ParsingError::InvalidLength {
                expected: SPOT_LENGTH,
                actual: 26,
            })
        ));
        assert!(matches!(
            "BTC_USD_BONJOURdid".parse::<EventId>(),
            Err(ParsingError::InvalidLength {
                expected: SPOT_LENGTH,
                actual: 18,
            })
        ));
    }
}
