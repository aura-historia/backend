use std::fmt::{Display, Formatter};

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct CrawlerDomainId(Uuid);

impl Display for CrawlerDomainId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for CrawlerDomainId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<CrawlerDomainId> for Uuid {
    fn from(value: CrawlerDomainId) -> Self {
        value.0
    }
}

impl TryFrom<String> for CrawlerDomainId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}

impl TryFrom<&str> for CrawlerDomainId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_serialize_as_a_uuid_string() {
        let uuid = Uuid::new_v4();
        let id = CrawlerDomainId::from(uuid);

        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{uuid}\""));
        assert_eq!(
            serde_json::from_str::<CrawlerDomainId>(&format!("\"{uuid}\"")).unwrap(),
            id
        );
    }
}
