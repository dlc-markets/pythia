use std::{fmt, str::FromStr};

use arrayvec::ArrayString;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{de::Visitor, Deserialize, Deserializer, Serialize};

use crate::data_models::error::ParsingError;

pub const EXPIRY_LENGTH: usize = 7;

#[derive(PartialEq, Eq, Clone, Copy, Debug, Hash, Serialize)]
#[serde(transparent)]
pub struct Expiry(
    // INVARIANT: must be a naive date of the form %d%b%y for chrono
    // The time part is always 8:00:00.000 UTC
    ArrayString<EXPIRY_LENGTH>,
);

impl AsRef<str> for Expiry {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl AsMut<str> for Expiry {
    fn as_mut(&mut self) -> &mut str {
        self.0.as_mut()
    }
}

impl fmt::Display for Expiry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

impl From<NaiveDate> for Expiry {
    fn from(value: NaiveDate) -> Self {
        let mut string = ArrayString::<EXPIRY_LENGTH>::new();
        value
            .format("%d%b%y")
            .write_to(&mut string)
            .expect("to be 7 ascii chars long");
        string.make_ascii_uppercase();
        Expiry(string)
    }
}

impl From<Expiry> for NaiveDate {
    fn from(value: Expiry) -> Self {
        NaiveDate::parse_from_str(value.as_ref(), "%d%b%y")
            .expect("Expiry is valid from our invariant")
    }
}

impl From<Expiry> for DateTime<Utc> {
    fn from(value: Expiry) -> Self {
        NaiveDate::from(value)
            .and_hms_milli_opt(8, 0, 0, 0)
            .expect("8 is valid hour and 0 is valid minute, second and millisecond")
            .and_utc()
    }
}

impl Ord for Expiry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let self_ts = NaiveDate::from(*self);
        let other_ts = NaiveDate::from(*other);

        self_ts.cmp(&other_ts)
    }
}

impl PartialOrd for Expiry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<'de> Deserialize<'de> for Expiry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Visitor to deserialize an expiry from a string
        /// while ensuring our invariant holds
        struct ExpiryVisitor;

        impl<'de> Visitor<'de> for ExpiryVisitor {
            type Value = Expiry;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string in the format %d%b%y uppercased")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse()
                    .map_err(|e| E::custom(format!("Error parsing expiry: {e}")))
            }
        }

        deserializer.deserialize_str(ExpiryVisitor)
    }
}

impl FromStr for Expiry {
    type Err = ParsingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let date = NaiveDate::parse_from_str(value, "%d%b%y").map_err(ParsingError::InvalidDate)?;
        Ok(if value.len() == 7 {
            Expiry(value.parse().expect("to be 7 ascii chars long"))
        } else {
            date.into()
        })
    }
}

#[cfg(test)]
mod tests_expiry {
    use chrono::Utc;

    use super::*;

    #[test]
    fn test_expiry_from_str() {
        let now = Utc::now();

        let today_expiry = Expiry::from(now.date_naive());
        let reparsed = today_expiry.as_ref().parse().unwrap();
        assert_eq!(today_expiry, reparsed);

        let with_zero_expiry = "01JUL25".parse::<Expiry>().unwrap();
        let missing_zero_expiry = "1JUL25".parse().unwrap();
        assert_eq!(with_zero_expiry, missing_zero_expiry);

        assert!("101JUL25".parse::<Expiry>().is_err());
        assert!("101JUL15425".parse::<Expiry>().is_err());
        assert!("01WTF23".parse::<Expiry>().is_err());
        assert!("Expiry".parse::<Expiry>().is_err());
    }

    #[test]
    fn test_expiry_cmp() {
        let expiry1 = "01JUL25".parse::<Expiry>().unwrap();
        let expiry2 = "02JUL25".parse().unwrap();
        assert_eq!(expiry1.cmp(&expiry2), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_expiry_serde() {
        let expiry = "01JUL25".parse::<Expiry>().unwrap();
        let serde_expiry = serde_json::to_string(&expiry).unwrap();
        assert_eq!(serde_expiry, "\"01JUL25\"");

        let de_ser_expiry: Expiry = serde_json::from_str(&serde_expiry).unwrap();
        assert_eq!(expiry, de_ser_expiry);
    }
}
