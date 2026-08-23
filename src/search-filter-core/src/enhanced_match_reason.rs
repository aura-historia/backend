#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct EnhancedMatchReason(String);

impl From<String> for EnhancedMatchReason {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&String> for EnhancedMatchReason {
    fn from(value: &String) -> Self {
        Self(value.to_owned())
    }
}

impl From<&str> for EnhancedMatchReason {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<EnhancedMatchReason> for String {
    fn from(value: EnhancedMatchReason) -> Self {
        value.0
    }
}

impl AsRef<str> for EnhancedMatchReason {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for EnhancedMatchReason {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EnhancedMatchReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}
