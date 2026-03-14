use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct NotificationId(Uuid);

impl Default for NotificationId {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for NotificationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for NotificationId {
    fn from(uuid: Uuid) -> Self {
        NotificationId(uuid)
    }
}

impl TryFrom<String> for NotificationId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<NotificationId> for String {
    fn from(id: NotificationId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for NotificationId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}
