//! Transactional email to an address that does not belong to a user account.
//!
//! A producer in any bounded context enqueues an external email in its own business
//! transaction and the notification delivery pipeline sends it; `docs/events/flow.md`
//! (External email delivery) owns the contract.

use strum::IntoEnumIterator;
use uuid::Uuid;

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
