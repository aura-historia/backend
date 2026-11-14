//! Transactional email to an address that does not belong to a user account.
//!
//! A producer in any bounded context enqueues an external email in its own business
//! transaction and the notification delivery pipeline sends it; `docs/events/flow.md`
//! (External email delivery) owns the contract.

use serde_email::Email;
use strum::IntoEnumIterator;
use uuid::Uuid;

/// The address an external email is sent to: a bare address without display text.
/// It is personal data, so `Debug` and errors never repeat it.
#[derive(Clone, PartialEq, Eq)]
pub struct ExternalEmailRecipient(Email);

/// Why a recipient address was rejected. It never repeats the address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidExternalEmailRecipient {
    #[error("email address is invalid")]
    Invalid,
    #[error("email address must be a bare address without display text, spaces or controls")]
    NotBareAddress,
    #[error("email address must not exceed {max_bytes} bytes")]
    TooLong { max_bytes: usize },
}

impl ExternalEmailRecipient {
    pub const MAX_BYTES: usize = 320;

    pub fn as_email(&self) -> &Email {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<&str> for ExternalEmailRecipient {
    type Error = InvalidExternalEmailRecipient;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.len() > Self::MAX_BYTES {
            return Err(InvalidExternalEmailRecipient::TooLong {
                max_bytes: Self::MAX_BYTES,
            });
        }
        if value.chars().any(|character| {
            matches!(character, '<' | '>') || character.is_whitespace() || character.is_control()
        }) {
            return Err(InvalidExternalEmailRecipient::NotBareAddress);
        }
        // The email error repeats the address, so it is not kept as the cause.
        Email::new(value)
            .map(Self)
            .map_err(|_| InvalidExternalEmailRecipient::Invalid)
    }
}

impl TryFrom<String> for ExternalEmailRecipient {
    type Error = InvalidExternalEmailRecipient;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl std::fmt::Debug for ExternalEmailRecipient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ExternalEmailRecipient")
            .field(&"<redacted>")
            .finish()
    }
}

/// The typed template variables of an external email. Each variant is one template kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEmailContent {
    /// Test-only kind that exercises the delivery mechanism. No producer enqueues it and
    /// no template is deployed for it, so a delivery of it fails permanently.
    TestProbe(ExternalEmailTestProbe),
}

impl ExternalEmailContent {
    pub const fn template(&self) -> ExternalEmailTemplate {
        match self {
            Self::TestProbe(_) => ExternalEmailTemplate::TestProbe,
        }
    }
}

/// The template kind of an external email, one per [`ExternalEmailContent`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ExternalEmailTemplate {
    TestProbe,
}

impl ExternalEmailTemplate {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TestProbe => "TEST_PROBE",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::iter().find(|template| template.as_str() == code)
    }
}

/// The business record that caused an external email. One origin enqueues each template
/// kind at most once, and erasing an origin erases its messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExternalEmailOrigin {
    /// Origin of the test-only [`ExternalEmailContent::TestProbe`] kind.
    TestProbe(Uuid),
}

impl ExternalEmailOrigin {
    pub const fn origin_type(self) -> &'static str {
        match self {
            Self::TestProbe(_) => "TEST_PROBE",
        }
    }

    pub const fn origin_id(self) -> Uuid {
        match self {
            Self::TestProbe(id) => id,
        }
    }
}

const TEST_PROBE_REFERENCE_MAX_CHARS: usize = 100;

/// Variables of the test-only kind: a short reference that must reach the rendered email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEmailTestProbe {
    reference: String,
}

impl ExternalEmailTestProbe {
    pub fn new(reference: &str) -> Result<Self, InvalidExternalEmailTestProbe> {
        let reference = reference.trim();
        if reference.is_empty()
            || reference.chars().count() > TEST_PROBE_REFERENCE_MAX_CHARS
            || reference.chars().any(char::is_control)
        {
            return Err(InvalidExternalEmailTestProbe);
        }
        Ok(Self {
            reference: reference.to_owned(),
        })
    }

    pub fn reference(&self) -> &str {
        &self.reference
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("external email test probe reference must be 1 to 100 printable characters")]
pub struct InvalidExternalEmailTestProbe;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_accept_a_trimmed_bare_recipient_address() -> Result<(), InvalidExternalEmailRecipient>
    {
        let recipient = ExternalEmailRecipient::try_from(" notifier@example.test ")?;

        assert_eq!("notifier@example.test", recipient.as_str());
        assert_eq!(
            "ExternalEmailRecipient(\"<redacted>\")",
            format!("{recipient:?}")
        );
        Ok(())
    }

    #[test]
    fn should_reject_recipients_that_are_not_bare_addresses_without_repeating_them() {
        let too_long = format!("{}@example.test", "a".repeat(320));
        for (value, expected) in [
            (
                "Jane <jane@example.test>",
                InvalidExternalEmailRecipient::NotBareAddress,
            ),
            (
                "\"jane doe\"@example.test",
                InvalidExternalEmailRecipient::NotBareAddress,
            ),
            ("not-an-address", InvalidExternalEmailRecipient::Invalid),
            (
                too_long.as_str(),
                InvalidExternalEmailRecipient::TooLong { max_bytes: 320 },
            ),
        ] {
            let error =
                ExternalEmailRecipient::try_from(value).expect_err("recipient should be rejected");
            assert_eq!(expected, error);
            assert!(!error.to_string().contains("example.test"));
        }
    }

    #[test]
    fn should_round_trip_every_template_code() {
        let templates = ExternalEmailTemplate::iter().collect::<Vec<_>>();
        let codes = templates
            .iter()
            .map(|template| template.as_str())
            .collect::<HashSet<_>>();

        assert_eq!(templates.len(), codes.len());
        for template in templates {
            assert_eq!(
                Some(template),
                ExternalEmailTemplate::from_code(template.as_str())
            );
        }
        assert_eq!(None, ExternalEmailTemplate::from_code("test_probe"));
    }

    #[test]
    fn should_name_the_template_of_the_content() -> Result<(), InvalidExternalEmailTestProbe> {
        let content = ExternalEmailContent::TestProbe(ExternalEmailTestProbe::new("probe-1")?);

        assert_eq!(ExternalEmailTemplate::TestProbe, content.template());
        Ok(())
    }

    #[test]
    fn should_trim_test_probe_reference() -> Result<(), InvalidExternalEmailTestProbe> {
        assert_eq!(
            "probe-1",
            ExternalEmailTestProbe::new(" probe-1\t")?.reference()
        );
        Ok(())
    }

    #[test]
    fn should_reject_blank_overlong_or_control_test_probe_reference() {
        for reference in [" ", &"x".repeat(101), "probe\n1"] {
            assert_eq!(
                Err(InvalidExternalEmailTestProbe),
                ExternalEmailTestProbe::new(reference)
            );
        }
    }

    #[test]
    fn should_expose_persisted_origin_identity() {
        let id = Uuid::now_v7();
        let origin = ExternalEmailOrigin::TestProbe(id);

        assert_eq!("TEST_PROBE", origin.origin_type());
        assert_eq!(id, origin.origin_id());
    }
}
