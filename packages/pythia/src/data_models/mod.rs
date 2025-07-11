use std::cell::LazyCell;

use arrayvec::ArrayString;
use serde::{Deserialize, Serialize};

use crate::data_models::error::ParsingError;

pub mod asset_pair;
pub mod error;
pub mod event_ids;
pub mod expiries;
pub mod oracle_msgs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Outcome(ArrayString<1>);

impl TryFrom<char> for Outcome {
    type Error = ParsingError;

    fn try_from(value: char) -> Result<Self, Self::Error> {
        if value.len_utf8() != 1 {
            return Err(ParsingError::InvalidLength {
                expected: 1,
                actual: value.len_utf8(),
            });
        }

        let mut result = ArrayString::<1>::new();
        result.push(value);

        Ok(Outcome(result))
    }
}

impl std::ops::Deref for Outcome {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

thread_local! {
    pub static OUTCOME_ZERO: LazyCell<Outcome> = LazyCell::new(|| Outcome(ArrayString::from("0").unwrap()));
    pub static OUTCOME_ONE: LazyCell<Outcome> = LazyCell::new(|| Outcome(ArrayString::from("1").unwrap()));
}
