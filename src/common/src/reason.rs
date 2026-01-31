use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Reason(String);

impl Display for Reason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Reason> for String {
    fn from(id: Reason) -> Self {
        id.0
    }
}

impl From<String> for Reason {
    fn from(value: String) -> Self {
        Reason(value)
    }
}

impl From<&String> for Reason {
    fn from(value: &String) -> Self {
        Reason(value.to_owned())
    }
}

impl From<&str> for Reason {
    fn from(value: &str) -> Self {
        Reason(value.to_owned())
    }
}
