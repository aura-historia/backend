use std::fmt::{Display, Formatter};

use percent_encoding::{AsciiSet, CONTROLS, NON_ALPHANUMERIC, utf8_percent_encode};
use url::Url;

const PARTNERIZE_DESTINATION: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'#')
    .add(b'%')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b':')
    .add(b'@')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b';')
    .add(b'=')
    .add(b'[')
    .add(b']');

/// Partnerize campaign reference used as the `camref` path component.
///
/// Partnerize references are ASCII alphanumeric identifiers (for example,
/// `1101l3AbC`). Values are preserved exactly: outer whitespace is rejected rather
/// than trimmed. The 128-byte limit bounds persisted JSON and outbound URL paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartnerizeCamref(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PartnerizeCamrefError {
    #[error("Partnerize camref must not be blank")]
    Blank,
    #[error("Partnerize camref must not exceed {max_bytes} bytes (got {actual_bytes})")]
    TooLong {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error("Partnerize camref must contain only ASCII letters and digits")]
    InvalidCharacters,
}

impl PartnerizeCamref {
    pub const MAX_BYTES: usize = 128;
}

impl TryFrom<&str> for PartnerizeCamref {
    type Error = PartnerizeCamrefError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(PartnerizeCamrefError::Blank);
        }

        let actual_bytes = value.len();
        if actual_bytes > Self::MAX_BYTES {
            return Err(PartnerizeCamrefError::TooLong {
                max_bytes: Self::MAX_BYTES,
                actual_bytes,
            });
        }

        if !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(PartnerizeCamrefError::InvalidCharacters);
        }

        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for PartnerizeCamref {
    type Error = PartnerizeCamrefError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for PartnerizeCamref {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for PartnerizeCamref {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferralConfiguration {
    Partnerize { camref: PartnerizeCamref },
}

#[derive(Debug, thiserror::Error)]
#[error("could not build referral URL")]
pub struct ReferralUrlError;

pub fn outbound_url(
    referral_configuration: Option<&ReferralConfiguration>,
    destination: &Url,
) -> Result<Url, ReferralUrlError> {
    match referral_configuration {
        Some(ReferralConfiguration::Partnerize { camref }) => Url::parse(&format!(
            "https://prf.hn/click/camref:{}/pubref:aurahistoria/destination:{}",
            utf8_percent_encode(camref.as_ref(), NON_ALPHANUMERIC),
            utf8_percent_encode(destination.as_str(), PARTNERIZE_DESTINATION)
        ))
        .map_err(|_| ReferralUrlError),
        None => Ok(append_aura_utm(destination.clone())),
    }
}

fn append_aura_utm(mut url: Url) -> Url {
    if !url.query_pairs().any(|(key, _)| key == "utm_source") {
        url.query_pairs_mut()
            .append_pair("utm_source", "aura_historia")
            .append_pair("utm_medium", "referral");
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_partnerize_camref_at_byte_cap_without_normalizing() {
        let value = "A".repeat(PartnerizeCamref::MAX_BYTES);

        assert_eq!(
            Ok(value.clone()),
            PartnerizeCamref::try_from(value.as_str()).map(|camref| camref.to_string())
        );
    }

    #[test]
    fn should_reject_unsafe_partnerize_camref_values() {
        for value in [
            "",
            " campaign",
            "campaign ",
            "campaign/ref",
            "campaign?ref",
            "campaign#ref",
            "campaign%ref",
            "campaign\tref",
            "campaign\nref",
            "café",
        ] {
            assert!(PartnerizeCamref::try_from(value).is_err(), "{value:?}");
        }

        let too_long = "A".repeat(PartnerizeCamref::MAX_BYTES + 1);
        assert_eq!(
            Err(PartnerizeCamrefError::TooLong {
                max_bytes: PartnerizeCamref::MAX_BYTES,
                actual_bytes: PartnerizeCamref::MAX_BYTES + 1,
            }),
            PartnerizeCamref::try_from(too_long.as_str())
        );
    }

    #[test]
    fn should_encode_partnerize_camref_defensively_when_building_outbound_url() {
        let destination = Url::parse("https://source.example/item?colour=red")
            .unwrap_or_else(|error| panic!("test URL: {error}"));
        let camref = PartnerizeCamref::try_from("1101l3AbC")
            .unwrap_or_else(|error| panic!("test camref: {error}"));

        let result = outbound_url(
            Some(&ReferralConfiguration::Partnerize { camref }),
            &destination,
        )
        .unwrap_or_else(|error| panic!("could not build referral URL: {error}"));

        assert_eq!(
            Url::parse(
                "https://prf.hn/click/camref:1101l3AbC/pubref:aurahistoria/destination:https%3A%2F%2Fsource.example%2Fitem%3Fcolour%3Dred",
            )
            .unwrap_or_else(|error| panic!("test URL: {error}")),
            result
        );
    }

    #[test]
    fn should_build_aura_utm_outbound_url_when_referral_is_not_configured() {
        let destination = Url::parse("https://source.example/item?colour=red")
            .unwrap_or_else(|error| panic!("test URL: {error}"));

        let result = outbound_url(None, &destination)
            .unwrap_or_else(|error| panic!("could not build referral URL: {error}"));

        assert_eq!(
            Url::parse(
                "https://source.example/item?colour=red&utm_source=aura_historia&utm_medium=referral",
            )
            .unwrap_or_else(|error| panic!("test URL: {error}")),
            result
        );
    }
}
