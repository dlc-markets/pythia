use serde::{
    de::{DeserializeSeed, Error, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::collections::{hash_map::Entry, HashMap};

#[derive(Debug, Clone)]
pub struct DeribitResponseForward<'a> {
    pub result: HashMap<&'a str, f64>,
}

struct UniquesCountDeserializer {
    nb_expiries: usize,
}

impl<'de> DeserializeSeed<'de> for UniquesCountDeserializer {
    type Value = HashMap<&'de str, f64>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
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
                let mut values = HashMap::with_capacity(self.nb_expiries);
                let mut count = 0;

                #[derive(Deserialize)]
                struct DeribitQuoteForwardResponse<'a> {
                    underlying_index: &'a str,
                    underlying_price: f64,
                }

                while let Some(quote) = access.next_element::<DeribitQuoteForwardResponse>()? {
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

                while let Some(IgnoredAny) = access.next_element()? {
                    // ignore
                }

                Ok(values)
            }
        }

        let visitor = SeqVisitor {
            nb_expiries: self.nb_expiries,
        };

        deserializer.deserialize_seq(visitor)
    }
}

pub struct DeribitResponseForwardDeserializer {
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
                            let value = access.next_value_seed(UniquesCountDeserializer {
                                nb_expiries: self.nb_expiries,
                            });
                            result = Some(value?);
                        }
                        _ => {
                            access.next_value::<IgnoredAny>()?;
                        }
                    }
                }

                match result {
                    Some(result) => Ok(DeribitResponseForward { result }),
                    None => Err(Error::missing_field("result")),
                }
            }
        }

        let visitor = ResultVisitor {
            nb_expiries: self.nb_expiries,
        };

        deserializer.deserialize_map(visitor)
    }
}
