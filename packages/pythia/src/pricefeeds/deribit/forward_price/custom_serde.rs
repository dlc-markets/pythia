use chrono::{DateTime, Utc};
use serde::{
    de::{Error, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::collections::{btree_map::Entry, BTreeMap};

use crate::data_models::expiries::Expiry;

use super::{DeribitErrorObject, DeribitResponseForward};

struct QuotesMap(BTreeMap<DateTime<Utc>, f64>);

impl<'de> Deserialize<'de> for QuotesMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Visitor that to deserialize the sequence of objects
        // returned by the Deribit API
        struct SeqVisitor;

        impl<'de> Visitor<'de> for SeqVisitor {
            type Value = QuotesMap;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an array of objects")
            }

            #[inline]
            fn visit_seq<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                // Use the number in self to allocate the hashmap beforehand.
                let mut values = BTreeMap::new();

                #[derive(Deserialize)]
                struct DeribitQuoteForwardResponse<'a> {
                    instrument_name: &'a str,
                    mark_price: f64,
                }

                while let Some(quote) = access.next_element::<DeribitQuoteForwardResponse>()? {
                    // `instrument_name` is always `BTC-[expiry]` with `expiry` being the expiry date in [DAY][MONTH][YEAR] format.
                    // If we are deserializing the perpetual quote conversion will fail and we just ignore it.
                    // We need to keep the expiry date in the same format and convert it to a `DateTime<Utc>`:
                    let Ok(expiry) = quote.instrument_name[4..].parse::<Expiry>() else {
                        continue;
                    };

                    if let Entry::Vacant(entry) = values.entry(expiry.into()) {
                        entry.insert(quote.mark_price);
                    }
                }

                Ok(QuotesMap(values))
            }
        }

        // Deserialize the sequence of objects using our visitor from the seed.
        deserializer.deserialize_seq(SeqVisitor)
    }
}

impl<'de> Deserialize<'de> for DeribitResponseForward {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Visitor to deserialize the JSON-RPC response
        struct ResultVisitor;

        impl<'de> Visitor<'de> for ResultVisitor {
            type Value = DeribitResponseForward;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a result field")
            }

            fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = None;
                while let Some(key) = access.next_key()? {
                    match key {
                        "result" => {
                            // We got a successful response, so we can pass the number of
                            // unique values we must expect to the deserializer of the result field.
                            let value = access.next_value::<QuotesMap>();
                            result = Some(value?);
                        }
                        "error" => {
                            let value = access.next_value::<DeribitErrorObject>()?;
                            return Ok(DeribitResponseForward::ErrorResponse { error: value });
                        }
                        _ => {
                            access.next_value::<IgnoredAny>()?;
                        }
                    }
                }

                match result {
                    Some(QuotesMap(result)) => {
                        Ok(DeribitResponseForward::ResultResponse { result })
                    }
                    None => Err(Error::missing_field("result")),
                }
            }
        }

        // Use our visitor to deserialize the JSON-RPC response:
        deserializer.deserialize_map(ResultVisitor)
    }
}
