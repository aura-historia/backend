use std::fmt::{Display, Formatter};

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid domain '{0}'")]
pub struct InvalidDomain(String);

impl InvalidDomain {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Domain(String);

impl Domain {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for Domain {
    type Error = InvalidDomain;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed != value || trimmed.contains(['/', '?', '#', '@', ':']) {
            return Err(InvalidDomain::new(value));
        }
        let url =
            Url::parse(&format!("https://{trimmed}")).map_err(|_| InvalidDomain::new(value))?;
        let host = url.host_str().ok_or_else(|| InvalidDomain::new(value))?;
        if host.contains('.') && url.port().is_none() {
            Ok(Self(host.to_ascii_lowercase()))
        } else {
            Err(InvalidDomain::new(value))
        }
    }
}

impl TryFrom<String> for Domain {
    type Error = InvalidDomain;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl Display for Domain {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
