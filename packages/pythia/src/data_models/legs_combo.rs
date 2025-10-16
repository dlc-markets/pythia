use std::{fmt::Display, io::Write, num::NonZeroI32};

use crate::data_models::{asset_pair::AssetPair, error::ParsingError, expiries::Expiry};

use bitcoin_hashes::{
    sha256t::{self, Tag},
    sha256t_tag,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Leg {
    Call { expiry: Expiry, strike: u32 },
    Put { expiry: Expiry, strike: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type)]
#[sqlx(type_name = "vanilla_option_type", rename_all = "lowercase")]
pub enum Vanilla {
    Call,
    Put,
}

impl Ord for Vanilla {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Vanilla::Call, Vanilla::Put) => std::cmp::Ordering::Greater,
            (Vanilla::Put, Vanilla::Call) => std::cmp::Ordering::Less,
            (Vanilla::Call, Vanilla::Call) | (Vanilla::Put, Vanilla::Put) => {
                std::cmp::Ordering::Equal
            }
        }
    }
}

impl PartialOrd for Vanilla {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Leg {
    pub fn expiry(&self) -> Expiry {
        match self {
            Leg::Call { expiry, .. } => *expiry,
            Leg::Put { expiry, .. } => *expiry,
        }
    }

    pub fn strike(&self) -> u32 {
        match self {
            Leg::Call { strike, .. } => *strike,
            Leg::Put { strike, .. } => *strike,
        }
    }

    pub fn vanilla(&self) -> Vanilla {
        if matches!(self, Leg::Call { .. }) {
            Vanilla::Call
        } else {
            Vanilla::Put
        }
    }

    pub fn new(expiry: Expiry, strike: u32, vanilla: Vanilla) -> Self {
        match vanilla {
            Vanilla::Call => Leg::Call { expiry, strike },
            Vanilla::Put => Leg::Put { expiry, strike },
        }
    }
}

impl std::fmt::Display for Leg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Leg::Call { expiry, strike } => write!(f, "{expiry}-{strike}C"),
            Leg::Put { expiry, strike } => write!(f, "{expiry}-{strike}P"),
        }
    }
}

impl Ord for Leg {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self {
            Leg::Call { expiry, strike } | Leg::Put { expiry, strike } => expiry
                .cmp(&other.expiry())
                .then(strike.cmp(&other.strike()))
                .then(self.vanilla().cmp(&other.vanilla())),
        }
    }
}

impl PartialOrd for Leg {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

type Notional = NonZeroI32;

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct LegsCombo(Box<[(Notional, Leg)]>);

const TRUNCATED_HASH_LENGTH: usize = 10;

pub struct LegsComboId([u8; TRUNCATED_HASH_LENGTH]);

impl LegsCombo {
    pub fn new(legs: &[(i32, Leg)]) -> Result<Self, ParsingError> {
        error::validate_leg_combo(legs)?;
        let legs = legs
            .iter()
            .copied()
            .map(|(n, l)| {
                (
                    NonZeroI32::new(n).expect("we already checked that notional is not zero"),
                    l,
                )
            })
            .collect::<Box<[_]>>();

        Ok(Self(legs))
    }

    pub fn combo_expiry(&self) -> Expiry {
        self.0.last().unwrap().1.expiry()
    }

    pub fn get_id_for_asset_pair(&self, asset_pair: AssetPair) -> LegsComboId {
        sha256t_tag! {
            /// Tagged hash for the legs combo id
            pub struct LegComboHash = hash_str("LEG_COMBO");
        }

        let mut hash_engine = LegComboHash::engine();
        let nb_legs = self.0.len();
        hash_engine
            .write_fmt(format_args!("{nb_legs}{asset_pair}"))
            .expect("write to hash engine cannot fail");
        for (notional, leg) in self.0.iter() {
            hash_engine
                .write_fmt(format_args!("{notional}x{leg}"))
                .expect("write to hash engine cannot fail");
        }
        LegsComboId(
            sha256t::Hash::<LegComboHash>::from_engine(hash_engine).to_byte_array()
                [..TRUNCATED_HASH_LENGTH]
                .try_into()
                .expect("Lengths were set with same TRUNCATED_HASH_LENGTH"),
        )
    }
}

pub const HRP_LEG_ID: &str = "LEGS";

const HRP_LEG_COMBO: bech32::Hrp = bech32::Hrp::parse_unchecked(HRP_LEG_ID);

impl Display for LegsComboId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        bech32::encode_upper_to_fmt::<bech32::NoChecksum, _>(f, HRP_LEG_COMBO, &self.0)
            .map_err(|_| std::fmt::Error::default())
    }
}

pub mod error {
    use displaydoc::Display;
    use gcd::Gcd as _;
    use std::cmp::Ordering;
    use thiserror::Error;

    use crate::data_models::legs_combo::{Leg, Vanilla};

    #[derive(Debug, Display, Error, PartialEq)]
    pub enum InvalidReason {
        /// the sum of calls notional is {0} which contradicts 1.
        SumOfCallsNegative(i32),
        /// the sum of puts notional is {0} which contradicts 2.
        SumOfPutsNegative(i32),
        /// the first leg has a negative notional of {0} which contradicts 3.
        FirstLegNotionalNotPositive(i32),
    }

    #[derive(Debug, Display, Error, PartialEq)]
    pub enum LegsComboError {
        /// Legs in the combo must be sorted by expiry then strike then put before call
        NotSortedLegCombo,

        /// Each leg of a combo must be unique
        NotUniqueLegCombo,

        /// Each leg of a combo must have a non zero notional
        NotZeroNotionalLegCombo,

        /// A leg combo cannot be empty
        EmptyLegCombo,

        /** All signs of notional must be inverted to get a valid representative combo.
        Inverting all signs of a leg combo result in a redundant one as it only reverts the sign of attested prices.
        We impose the following to remove any ambiguity on the signs of notional and keep only one representative combo:
        1. The sum of call notional must be non negative.
        2. If it is zero, then the sum of put notional must be non negative.
        3. If it is also zero, then the first leg must have a positive notional.

        Provided combo was invalid because {0}*/
        InvertedSignsOfCombo(InvalidReason),

        /// Too many legs: {0}, max is 65535
        TooManyLegs(usize),

        /// The legs notional must have a gcd of 1 but it is {0}
        GcdNotOne(u32),
    }

    pub(super) fn validate_leg_combo(legs: &[(i32, Leg)]) -> Result<(), LegsComboError> {
        // Fail if legs are not sorted or if there are duplicate legs
        if !legs.iter().is_sorted_by(|(_, a), (_, b)| a < b) {
            return Err(LegsComboError::NotSortedLegCombo);
        }

        // Compute gcd and the sum of call notional to decide if the combo is valid:
        let (gcd, sum_call) = legs.iter().try_fold(
            (0_u32, 0_i32),
            |(gcd_current, sum_call), &(notional, leg)| {
                Ok((
                    gcd_current.gcd(notional.unsigned_abs()),
                    if notional == 0 {
                        return Err(LegsComboError::NotZeroNotionalLegCombo);
                    } else {
                        sum_call + notional * (leg.vanilla() == Vanilla::Call) as i32
                    },
                ))
            },
        )?;

        // Case where the gcd is not 1, user must divide all notional by the gcd
        if gcd > 1 {
            return Err(LegsComboError::GcdNotOne(gcd));
        }

        // Case where legs is empty which is equivalent to gcd is 0
        if gcd == 0 {
            return Err(LegsComboError::EmptyLegCombo);
        }

        // Case where caller input a combo where the mark price is decreasing at infinity
        // which we consider invalid, user must invert all signs t get a correct combo
        match sum_call.cmp(&0) {
            // If amount of call is positive, we are sure this is our representative combo
            Ordering::Greater => Ok(()),
            // If amount of call is negative, we know user needs to invert all signs
            Ordering::Less => Err(LegsComboError::InvertedSignsOfCombo(
                InvalidReason::SumOfCallsNegative(sum_call),
            )),
            // Ambiguous case, we have to check the sum of put notional
            Ordering::Equal => {
                // We require the sum of puts notional to be non negative
                match legs.iter().copied().map(|(n, _)| n).sum::<i32>() {
                    // It is negative, we are sure user needs to invert all signs
                    put_sum @ ..0 => Err(LegsComboError::InvertedSignsOfCombo(
                        InvalidReason::SumOfPutsNegative(put_sum),
                    )),
                    1.. => {
                        // Positive notional of puts: this is a valid combo
                        Ok(())
                    }
                    // One left ambiguity case where the notional of puts also cancel out.
                    0 if legs.first().expect("we ruled out no legs already").0 < 0 => {
                        // We resolve ambiguity by asking that the first leg has a positive notional
                        Err(LegsComboError::InvertedSignsOfCombo(
                            InvalidReason::FirstLegNotionalNotPositive(
                                legs.first().expect("we ruled out no legs already").0,
                            ),
                        ))
                    }
                    // A first leg notional can neither be 0 nor negative: this is a valid combo
                    0 => Ok(()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data_models::legs_combo::error::{InvalidReason, LegsComboError};

    use super::*;

    use bitcoin_hashes::sha256::{Hash, HashEngine, Midstate};

    #[test]
    fn test_hashing_matches() {
        let legs_combo = LegsCombo::new(&mut vec![
            (
                -1,
                Leg::Put {
                    expiry: "01Jan25".parse().unwrap(),
                    strike: 90000,
                },
            ),
            (
                2,
                Leg::Call {
                    expiry: "01APR25".parse().unwrap(),
                    strike: 100000,
                },
            ),
        ])
        .unwrap();
        let str_to_hash = format!(
            "{}{}{}",
            legs_combo.0.len(),
            AssetPair::BtcUsd,
            legs_combo
                .0
                .iter()
                .map(|(notional, leg)| format!("{notional}x{leg}"))
                .collect::<Vec<_>>()
                .join("")
        );
        assert_eq!(str_to_hash, "2btc_usd-1x01JAN25-90000P2x01APR25-100000C");

        let mut hash_engine = HashEngine::from_midstate(Midstate::hash_tag("LEG_COMBO".as_bytes()));

        hash_engine
            .write_fmt(format_args!("{str_to_hash}"))
            .expect("write to hash engine cannot fail");

        let hash = Hash::from_engine(hash_engine);
        assert_eq!(
            hash.to_byte_array()[..TRUNCATED_HASH_LENGTH],
            legs_combo.get_id_for_asset_pair(AssetPair::BtcUsd).0
        );
    }

    #[test]
    fn test_one_leg_combo_id() {
        let legs_combo = LegsCombo::new(&mut vec![(
            1,
            Leg::Call {
                expiry: "01Jan25".parse().unwrap(),
                strike: 100000,
            },
        )])
        .unwrap();
        let id = legs_combo.get_id_for_asset_pair(AssetPair::BtcUsd);
        assert_eq!(id.to_string(), "LEGS1J06FUXV24GRN8VLR");

        let legs_combo = LegsCombo::new(&mut vec![(
            1,
            Leg::Put {
                expiry: "24NOV25".parse().unwrap(),
                strike: 200000,
            },
        )])
        .unwrap();
        let id = legs_combo.get_id_for_asset_pair(AssetPair::BtcUsd);
        assert_eq!(id.to_string(), "LEGS1UUDR48M4G9MRWR2N");
    }

    #[test]
    fn test_out_of_order_legs_fails() {
        assert_eq!(
            LegsCombo::new(&mut vec![
                (
                    -2,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 140000,
                    },
                ),
                (
                    1,
                    Leg::Put {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 100000,
                    },
                ),
            ]),
            Err(LegsComboError::NotSortedLegCombo.into())
        );
    }

    #[test]
    fn test_negative_calls_fails() {
        assert_eq!(
            LegsCombo::new(&mut vec![
                (
                    1,
                    Leg::Put {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 100000,
                    },
                ),
                (
                    -2,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 140000,
                    },
                ),
            ])
            .inspect(|e| println!("{:?}", e)),
            Err(LegsComboError::InvertedSignsOfCombo(InvalidReason::SumOfCallsNegative(-2)).into())
        );
    }

    #[test]
    fn test_negative_puts_fails() {
        assert_eq!(
            LegsCombo::new(&mut vec![
                (
                    -1,
                    Leg::Put {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 100000,
                    },
                ),
                (
                    -2,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 140000,
                    },
                ),
                (
                    2,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 200000,
                    },
                ),
            ]),
            Err(LegsComboError::InvertedSignsOfCombo(InvalidReason::SumOfPutsNegative(-1)).into())
        );
    }

    #[test]
    fn test_fail_multiple_of_a_legs_combo() {
        assert_eq!(
            LegsCombo::new(&mut vec![
                (
                    2,
                    Leg::Put {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 100000,
                    },
                ),
                (
                    -4,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 140000,
                    },
                ),
            ]),
            Err(LegsComboError::GcdNotOne(2).into())
        );
    }

    #[test]
    fn test_no_zero_notional() {
        LegsCombo::new(&mut vec![(
            0,
            Leg::Call {
                expiry: "01Jan25".parse().unwrap(),
                strike: 100000,
            },
        )])
        .unwrap_err();
    }

    #[test]
    fn test_no_duplicate_legs() {
        assert_eq!(
            LegsCombo::new(&mut vec![
                (
                    1,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 100000,
                    },
                ),
                (
                    1,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 100000,
                    },
                ),
            ]),
            Err(LegsComboError::NotUniqueLegCombo.into())
        );
    }

    #[test]
    fn test_not_empty() {
        LegsCombo::new(&mut vec![]).unwrap_err();
    }

    #[test]
    fn test_first_leg_sign_ambiguity() {
        assert_eq!(
            LegsCombo::new(&mut vec![
                (
                    -1,
                    Leg::Put {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 80000,
                    },
                ),
                (
                    1,
                    Leg::Put {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 90000,
                    },
                ),
                (
                    1,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 110000,
                    },
                ),
                (
                    -1,
                    Leg::Call {
                        expiry: "01Jan25".parse().unwrap(),
                        strike: 120000,
                    },
                ),
            ])
            .inspect_err(|e| println!("{}", e)),
            Err(
                LegsComboError::InvertedSignsOfCombo(InvalidReason::FirstLegNotionalNotPositive(
                    -1
                ))
                .into()
            )
        );

        let leg_combo = LegsCombo::new(&mut vec![
            (
                1,
                Leg::Put {
                    expiry: "01Jan25".parse().unwrap(),
                    strike: 80000,
                },
            ),
            (
                -1,
                Leg::Put {
                    expiry: "01Jan25".parse().unwrap(),
                    strike: 90000,
                },
            ),
            (
                -1,
                Leg::Call {
                    expiry: "01Jan25".parse().unwrap(),
                    strike: 110000,
                },
            ),
            (
                1,
                Leg::Call {
                    expiry: "01Jan25".parse().unwrap(),
                    strike: 120000,
                },
            ),
        ])
        .unwrap();

        let id = leg_combo.get_id_for_asset_pair(AssetPair::BtcUsd);
        assert_eq!(id.to_string(), "LEGS1FFS9DUM2A7EK97QF");
    }
}
