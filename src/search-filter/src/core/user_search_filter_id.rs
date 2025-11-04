use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct UserSearchFilterId(Uuid);

impl Default for UserSearchFilterId {
    fn default() -> Self {
        Self::new()
    }
}

impl UserSearchFilterId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for UserSearchFilterId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for UserSearchFilterId {
    fn from(uuid: Uuid) -> Self {
        UserSearchFilterId(uuid)
    }
}

impl TryFrom<String> for UserSearchFilterId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<UserSearchFilterId> for String {
    fn from(id: UserSearchFilterId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for UserSearchFilterId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user_search_filter_id::UserSearchFilterId;
    use fake::Dummy;

    impl<T> Dummy<T> for UserSearchFilterId {
        fn dummy_with_rng<R: fake::Rng + ?Sized>(_config: &T, _rng: &mut R) -> Self {
            Default::default()
        }
    }
}
