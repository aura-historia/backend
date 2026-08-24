use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct OAuthClientId(Uuid);

impl Default for OAuthClientId {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuthClientId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Unix seconds encoded in this identifier's UUIDv7 timestamp field.
    ///
    /// IDs created by [`Self::new`] use UUIDv7.
    pub fn issued_at_unix_timestamp(&self) -> i64 {
        let bytes = self.0.as_bytes();
        let milliseconds = u64::from_be_bytes([
            0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
        ]);
        (milliseconds / 1_000) as i64
    }
}

impl std::fmt::Display for OAuthClientId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl From<Uuid> for OAuthClientId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl TryFrom<String> for OAuthClientId {
    type Error = uuid::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value).map(Self)
    }
}

impl From<OAuthClientId> for String {
    fn from(value: OAuthClientId) -> Self {
        value.0.to_string()
    }
}

impl TryFrom<&str> for OAuthClientId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

impl TryFrom<&String> for OAuthClientId {
    type Error = uuid::Error;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::OAuthClientId;
    use uuid::Uuid;

    fn uuid_v7_with_milliseconds(milliseconds: u64) -> Uuid {
        let mut bytes = [0_u8; 16];
        bytes[..6].copy_from_slice(&milliseconds.to_be_bytes()[2..]);
        bytes[6] = 0x70;
        bytes[8] = 0x80;
        Uuid::from_bytes(bytes)
    }

    #[test]
    fn should_decode_uuid_v7_timestamp_as_unix_seconds() {
        let id = OAuthClientId::from(uuid_v7_with_milliseconds(1_700_000_123_456));

        assert_eq!(1_700_000_123, id.issued_at_unix_timestamp());
    }

    #[test]
    fn should_truncate_uuid_v7_timestamp_subseconds() {
        let id = OAuthClientId::from(uuid_v7_with_milliseconds(1_700_000_999_999));

        assert_eq!(1_700_000_999, id.issued_at_unix_timestamp());
    }
}
