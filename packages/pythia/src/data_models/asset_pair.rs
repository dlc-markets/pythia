use std::{
    fmt::{self, Display, Formatter},
    ops::Deref,
    str::FromStr,
};

use serde::{de::Visitor, Deserialize, Serialize};

#[cfg(test)]
use strum::EnumIter;

use crate::data_models::{error::ParsingError, ArrayString};

#[derive(Copy, Clone, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[cfg_attr(test, derive(EnumIter))]
#[serde(rename_all = "snake_case")]
pub enum AssetPair {
    #[default]
    BtcUsd,
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum Unit {
    ThreeLetterPair(ArrayString<7>),
}

impl Display for AssetPair {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            AssetPair::BtcUsd => write!(f, "btc_usd"),
        }
    }
}

impl Deref for Unit {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            Unit::ThreeLetterPair(s) => s,
        }
    }
}

impl FromStr for Unit {
    type Err = ParsingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Unit::ThreeLetterPair(s.parse().map_err(|_| {
            ParsingError::InvalidLength {
                expected: 7,
                actual: s.len(),
            }
        })?))
    }
}

impl Serialize for Unit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self)
    }
}

impl<'de> Deserialize<'de> for Unit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct UnitVisitor;

        impl<'de> Visitor<'de> for UnitVisitor {
            type Value = Unit;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string of length 7")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse().map_err(|_| E::invalid_length(v.len(), &"7"))
            }
        }
        deserializer.deserialize_str(UnitVisitor)
    }
}
