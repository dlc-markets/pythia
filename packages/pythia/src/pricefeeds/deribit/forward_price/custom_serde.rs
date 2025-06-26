use serde::{
    de::{DeserializeSeed, Error, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::collections::{hash_map::Entry, HashMap};

use super::{DeribitErrorObject, DeribitResponseForward};

// A seed deserializer that knows how many unique values we must expect to get from the
// Deribit API. This is used to avoid over-allocating and ignore all objects once it has a
// response for the expected number of quoting expiries.
struct UniquesCountDeserializer {
    nb_expiries: usize,
}

impl<'de> DeserializeSeed<'de> for UniquesCountDeserializer {
    type Value = HashMap<&'de str, f64>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Visitor that will be used to deserialize the sequence of objects
        // returned by the Deribit API using information provided by the seed.
        struct SeqVisitor {
            nb_expiries: usize,
        }

        impl<'de> Visitor<'de> for SeqVisitor {
            type Value = HashMap<&'de str, f64>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an array of objects")
            }

            #[inline]
            fn visit_seq<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                // Use the number in self to allocate the hashmap beforehand.
                let mut values = HashMap::with_capacity(self.nb_expiries);
                let mut count = 0;

                #[derive(Deserialize)]
                struct DeribitQuoteForwardResponse<'a> {
                    underlying_index: &'a str,
                    underlying_price: f64,
                }

                // Deserialize each object until we find the expected number of unique values.
                while let Some(quote) = access.next_element::<DeribitQuoteForwardResponse>()? {
                    // `underlying_index` is always `BTC-[expiry]` with `expiry` being the expiry date in [DAY][MONTH][YEAR] format.
                    // If the underlying future is synthetic it is prefixed with `SYN.`
                    // We only need to keep the expiry date in the same format:
                    let expiry = if &quote.underlying_index[0..4] == "SYN." {
                        &quote.underlying_index[8..]
                    } else {
                        &quote.underlying_index[4..]
                    };

                    if let Entry::Vacant(entry) = values.entry(expiry) {
                        entry.insert(quote.underlying_price);
                        count += 1;
                        if count == self.nb_expiries {
                            break;
                        }
                    }
                }

                // Ignore all other objects once we got all we need.
                while let Some(IgnoredAny) = access.next_element()? {
                    // ignore
                }

                Ok(values)
            }
        }

        let visitor = SeqVisitor {
            nb_expiries: self.nb_expiries,
        };

        // Deserialize the sequence of objects using our visitor from the seed.
        deserializer.deserialize_seq(visitor)
    }
}

// The sequence of object is wrapped in JSON-RPC format, so we need to deserialize it
// using a visitor that knows how many unique values we must expect in result field.
pub(super) struct DeribitResponseForwardDeserializer {
    nb_expiries: usize,
}

impl DeribitResponseForwardDeserializer {
    pub fn new(nb_expiries: usize) -> Self {
        Self { nb_expiries }
    }
}

impl<'de> DeserializeSeed<'de> for DeribitResponseForwardDeserializer {
    type Value = DeribitResponseForward<'de>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Visitor that will be used to deserialize the JSON-RPC response and pass its info
        // to the seed deserializer for the result field in successful case.
        struct ResultVisitor {
            nb_expiries: usize,
        }

        impl<'de> Visitor<'de> for ResultVisitor {
            type Value = DeribitResponseForward<'de>;

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
                            let value = access.next_value_seed(UniquesCountDeserializer {
                                nb_expiries: self.nb_expiries,
                            });
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
                    Some(result) => Ok(DeribitResponseForward::ResultResponse { result }),
                    None => Err(Error::missing_field("result")),
                }
            }
        }

        let visitor = ResultVisitor {
            nb_expiries: self.nb_expiries,
        };

        // Use our seeded visitor to deserialize the JSON-RPC response:
        deserializer.deserialize_map(visitor)
    }
}
