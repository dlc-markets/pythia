use std::{error::Error, fmt, ops::Deref, str::FromStr};

use arrayvec::ArrayString;
use chrono::{DateTime, Utc};
use serde::{de::Visitor, Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    postgres::{PgHasArrayType, PgTypeInfo},
    Database, Postgres,
};

use crate::data_models::{
    asset_pair::AssetPair,
    error::ParsingError,
    expiries::{Expiry, EXPIRY_LENGTH},
};

use sqlx::prelude::*;

const SPOT_LENGTH: usize = 17;
const FORWARD_LENGTH: usize = 26;
const DELIVERY_LENGTH: usize = 16;
const EXPIRY_OFFSET: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventId {
    Spot(ArrayString<SPOT_LENGTH>),
    // INVARIANT: at EXPIRY_OFFSET, there is a valid expiry written
    Forward(ArrayString<FORWARD_LENGTH>),
    // INVARIANT: at EXPIRY_OFFSET, there is a valid expiry written
    Delivery(ArrayString<DELIVERY_LENGTH>),
}

impl EventId {
    pub fn spot_from_pair_and_timestamp(pair: AssetPair, timestamp: DateTime<Utc>) -> Self {
        Self::Spot(
            format_args!("{pair}{t}", t = timestamp.timestamp())
                .try_into()
                .expect("We imposed a length of SPOT_LENGTH"),
        )
    }

    pub fn forward_of_expiry_with_pair_at_timestamp(
        pair: AssetPair,
        expiry: Expiry,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self::Forward(
            format_args!("{pair}_{expiry}f{t}", t = timestamp.timestamp())
                .try_into()
                .expect("We imposed a length of FORWARD_LENGTH"),
        )
    }

    pub fn delivery_of_expiry_with_pair(pair: AssetPair, expiry: Expiry) -> Self {
        Self::Delivery(
            format_args!("{pair}_{expiry}d")
                .try_into()
                .expect("We imposed a length of DELIVERY_LENGTH"),
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
            Self::Forward(s) => s.as_ref(),
            Self::Delivery(s) => s.as_ref(),
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
        match value {
            EventId::Forward(string) => {
                // INVARIANT used: at EXPIRY_OFFSET, there is a valid expiry written
                string[EXPIRY_OFFSET..EXPIRY_OFFSET + EXPIRY_LENGTH].parse()
            }
            EventId::Delivery(string) => {
                // INVARIANT used: at EXPIRY_OFFSET, there is a valid expiry written
                string[EXPIRY_OFFSET..EXPIRY_OFFSET + EXPIRY_LENGTH].parse()
            }
            _ => Err(ParsingError::NoExpiryInEventId),
        }
    }
}

impl FromStr for EventId {
    type Err = ParsingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let expiry_range = EXPIRY_OFFSET..EXPIRY_OFFSET + EXPIRY_LENGTH;

        if value.len() < expiry_range.end {
            return Err(ParsingError::InvalidLength {
                expected: SPOT_LENGTH,
                actual: value.len(),
            });
        }

        // INVARIANT check: at EXPIRY_RANGE, there must be a valid expiry
        if value[expiry_range].parse::<Expiry>().is_ok() {
            if let Ok(id) = value.parse::<ArrayString<DELIVERY_LENGTH>>() {
                return Ok(EventId::Delivery(id));
            }

            if let Ok(id) = value.parse::<ArrayString<FORWARD_LENGTH>>() {
                return Ok(EventId::Forward(id));
            }
        }

        if let Ok(id) = value.parse::<ArrayString<SPOT_LENGTH>>() {
            return Ok(EventId::Spot(id));
        }

        Err(ParsingError::InvalidLength {
            expected: SPOT_LENGTH,
            actual: value.len(),
        })
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
                v.parse::<EventId>()
                    .map_err(|l| E::custom(format!("Invalid event id: {l}")))
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
        let event_id_f1 = "BTC_USD_01JUL25f1717336000".parse::<EventId>().unwrap();
        let event_id_f2 = "BTC_USD_02JUL25f1717336000".parse::<EventId>().unwrap();

        assert!(matches!(event_id_f1, EventId::Forward(_)));
        assert!(matches!(event_id_f2, EventId::Forward(_)));

        let event_id_d1 = "BTC_USD_01JUL25d".parse::<EventId>().unwrap();
        let event_id_d2 = "BTC_USD_02JUL25d".parse::<EventId>().unwrap();

        assert!(matches!(event_id_d1, EventId::Delivery(_)));
        assert!(matches!(event_id_d2, EventId::Delivery(_)));

        let event_id_s1 = "BTC_USD1717336000".parse::<EventId>().unwrap();
        let event_id_s2 = "BTC_USD1717336000".parse::<EventId>().unwrap();

        assert!(matches!(event_id_s1, EventId::Spot(_)));
        assert!(matches!(event_id_s2, EventId::Spot(_)));
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
