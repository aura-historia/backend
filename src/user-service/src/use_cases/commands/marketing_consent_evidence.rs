use crate::ports::marketing_consent_recipient_key;
use application::operation_context::OperationContext;
use localization::Language;
use serde_email::Email;
use time::OffsetDateTime;
use user_core::user_id::UserId;

const PURPOSE: &str = "EMAIL_MARKETING";
const EVENT_NAME: &str = "marketing_consent.evidence.v1";
// Stable reference key; the reviewed fixed signup/DOI copy must stay bound to it in frontend source.
const WORDING_REFERENCE: &str = "webapp:marketing-email-consent:v1";

#[derive(Clone, Copy, Debug)]
pub(crate) enum ConsentEvidenceSource {
    CognitoSignup,
    AuraDoubleOptIn,
    UserWithdrawal,
    AccountDeleted,
    LoopsUserPreference,
    ProviderContactRemoved,
}

impl ConsentEvidenceSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CognitoSignup => "COGNITO_SIGNUP",
            Self::AuraDoubleOptIn => "AURA_DOUBLE_OPT_IN",
            Self::UserWithdrawal => "USER_WITHDRAWAL",
            Self::AccountDeleted => "ACCOUNT_DELETED",
            Self::LoopsUserPreference => "LOOPS_USER_PREFERENCE",
            Self::ProviderContactRemoved => "PROVIDER_CONTACT_REMOVED",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ConsentEvidenceAction {
    Grant,
    Revoke,
    Remove,
    Resubscribe,
}

impl ConsentEvidenceAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Grant => "GRANT",
            Self::Revoke => "REVOKE",
            Self::Remove => "REMOVE",
            Self::Resubscribe => "RESUBSCRIBE",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ConsentSubjectKind {
    User,
    EmailOnly,
}

impl ConsentSubjectKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::EmailOnly => "EMAIL_ONLY",
        }
    }
}

/// Bounded, log-safe event data created only from validated service decisions.
/// It deliberately contains neither an email address nor a source/proof key.
pub(crate) struct MarketingConsentEvidence {
    source: ConsentEvidenceSource,
    action: ConsentEvidenceAction,
    subject_kind: ConsentSubjectKind,
    user_id: Option<UserId>,
    recipient_fingerprint: String,
    previous_consent: Option<bool>,
    current_consent: Option<bool>,
    decision_id: String,
    consent_revision: Option<i64>,
    effective_at: OffsetDateTime,
    wording_reference: &'static str,
    wording_locale: &'static str,
}

impl MarketingConsentEvidence {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn user_transition(
        source: ConsentEvidenceSource,
        action: ConsentEvidenceAction,
        user_id: UserId,
        email: &Email,
        previous_consent: bool,
        current_consent: bool,
        decision_id: String,
        consent_revision: i64,
        effective_at: OffsetDateTime,
        wording_locale: &'static str,
    ) -> Self {
        Self {
            source,
            action,
            subject_kind: ConsentSubjectKind::User,
            user_id: Some(user_id),
            recipient_fingerprint: marketing_consent_recipient_key(email),
            previous_consent: Some(previous_consent),
            current_consent: Some(current_consent),
            decision_id,
            consent_revision: Some(consent_revision),
            effective_at,
            wording_reference: WORDING_REFERENCE,
            wording_locale,
        }
    }

    pub(crate) fn email_only(
        source: ConsentEvidenceSource,
        action: ConsentEvidenceAction,
        email: &Email,
        decision_id: String,
        effective_at: OffsetDateTime,
        wording_locale: &'static str,
    ) -> Self {
        Self {
            source,
            action,
            subject_kind: ConsentSubjectKind::EmailOnly,
            user_id: None,
            recipient_fingerprint: marketing_consent_recipient_key(email),
            previous_consent: None,
            current_consent: None,
            decision_id,
            consent_revision: None,
            effective_at,
            wording_reference: WORDING_REFERENCE,
            wording_locale,
        }
    }

    pub(crate) fn with_wording_locale(mut self, locale: &'static str) -> Self {
        self.wording_locale = locale;
        self
    }

    pub(crate) fn with_wording_reference(mut self, reference: &'static str) -> Self {
        self.wording_reference = reference;
        self
    }

    pub(crate) fn emit_after_commit(&self, context: Option<&OperationContext>) {
        let recorded_at = OffsetDateTime::now_utc();
        let effective_at = format_utc(self.effective_at);
        let recorded_at = format_utc(recorded_at);
        let request_id = context
            .map(|context| context.request_id.as_str())
            .filter(|value| safe_log_identifier(value));
        let correlation_id = context
            .map(|context| context.correlation_id.as_str())
            .filter(|value| safe_log_identifier(value));
        let release_sha = backend_release_sha();

        if let (Some(user_id), Some(previous), Some(current), Some(revision)) = (
            self.user_id,
            self.previous_consent,
            self.current_consent,
            self.consent_revision,
        ) {
            tracing::info!(
                event = EVENT_NAME,
                consent_purpose = PURPOSE,
                consent_source = self.source.as_str(),
                consent_action = self.action.as_str(),
                subject_kind = self.subject_kind.as_str(),
                user_id = %user_id,
                recipient_fingerprint = %self.recipient_fingerprint,
                previous_consent = previous,
                current_consent = current,
                consent_decision_id = %self.decision_id,
                consent_revision = revision,
                consent_effective_at_utc = %effective_at,
                consent_recorded_at_utc = %recorded_at,
                consent_wording_reference = self.wording_reference,
                consent_wording_locale = self.wording_locale,
                backend_release_sha = %release_sha,
                request_id = request_id.unwrap_or(""),
                correlation_id = correlation_id.unwrap_or(""),
                "Committed marketing consent evidence."
            );
        } else {
            tracing::info!(
                event = EVENT_NAME,
                consent_purpose = PURPOSE,
                consent_source = self.source.as_str(),
                consent_action = self.action.as_str(),
                subject_kind = self.subject_kind.as_str(),
                recipient_fingerprint = %self.recipient_fingerprint,
                consent_decision_id = %self.decision_id,
                consent_effective_at_utc = %effective_at,
                consent_recorded_at_utc = %recorded_at,
                consent_wording_reference = self.wording_reference,
                consent_wording_locale = self.wording_locale,
                backend_release_sha = %release_sha,
                request_id = request_id.unwrap_or(""),
                correlation_id = correlation_id.unwrap_or(""),
                "Committed marketing consent evidence."
            );
        }
    }
}

pub(crate) fn wording_locale(language: Option<Language>) -> &'static str {
    language.map(Language::as_str).unwrap_or("en")
}

fn backend_release_sha() -> String {
    std::env::var("BACKEND_RELEASE_SHA")
        .ok()
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn safe_log_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn format_utc(value: OffsetDateTime) -> String {
    value
        .to_offset(time::UtcOffset::UTC)
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp has a valid RFC3339 representation")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_source_and_action_labels_are_stable() {
        assert_eq!(
            ConsentEvidenceSource::CognitoSignup.as_str(),
            "COGNITO_SIGNUP"
        );
        assert_eq!(
            ConsentEvidenceSource::AuraDoubleOptIn.as_str(),
            "AURA_DOUBLE_OPT_IN"
        );
        assert_eq!(
            ConsentEvidenceSource::UserWithdrawal.as_str(),
            "USER_WITHDRAWAL"
        );
        assert_eq!(
            ConsentEvidenceSource::AccountDeleted.as_str(),
            "ACCOUNT_DELETED"
        );
        assert_eq!(
            ConsentEvidenceSource::LoopsUserPreference.as_str(),
            "LOOPS_USER_PREFERENCE"
        );
        assert_eq!(
            ConsentEvidenceSource::ProviderContactRemoved.as_str(),
            "PROVIDER_CONTACT_REMOVED"
        );
        assert_eq!(ConsentEvidenceAction::Grant.as_str(), "GRANT");
        assert_eq!(ConsentEvidenceAction::Revoke.as_str(), "REVOKE");
        assert_eq!(ConsentEvidenceAction::Remove.as_str(), "REMOVE");
        assert_eq!(ConsentEvidenceAction::Resubscribe.as_str(), "RESUBSCRIBE");
    }

    #[test]
    fn validated_locale_codes_are_used_and_missing_locale_is_english() {
        assert_eq!(wording_locale(Some(Language::De)), "de");
        assert_eq!(wording_locale(None), "en");
    }
}
