//! A human's label of a ListingSource the crawler should not crawl: a site that is not an online
//! shop, or one the crawler's approach cannot scrape. Labelling switches crawling off and the
//! scope sync keeps it off until a human clears the label (`docs/crawler/README.md#listingsource-scope`).

use super::ReviewActor;
use serde::{Deserialize, Serialize, Serializer};
use time::OffsetDateTime;

/// The longest label note, in characters.
pub const MAX_SOURCE_LABEL_NOTE_CHARS: usize = 500;

/// Why a ListingSource is not crawled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLabel {
    /// The site shows items but sells nothing online, such as a gallery's catalogue.
    NotAnEshop,
    /// The shop sells online, but the crawler's approach does not fit how it is built.
    ApproachNotApplicable,
    /// A bot protection keeps the crawler out.
    BotProtection,
    /// The product pages are rendered by JavaScript the crawler does not run.
    JavascriptRendered,
    /// The products are behind a login.
    LoginRequired,
    /// Any other cause; the note names it.
    Other,
}

impl SourceLabel {
    pub const ALL: [Self; 6] = [
        Self::NotAnEshop,
        Self::ApproachNotApplicable,
        Self::BotProtection,
        Self::JavascriptRendered,
        Self::LoginRequired,
        Self::Other,
    ];

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotAnEshop => "NOT_AN_ESHOP",
            Self::ApproachNotApplicable => "APPROACH_NOT_APPLICABLE",
            Self::BotProtection => "BOT_PROTECTION",
            Self::JavascriptRendered => "JAVASCRIPT_RENDERED",
            Self::LoginRequired => "LOGIN_REQUIRED",
            Self::Other => "OTHER",
        }
    }

    /// The label a persisted or requested name stands for, or `None` for a name no label has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|label| label.as_str() == value)
    }
}

impl Serialize for SourceLabel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SourceLabel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_persisted(&value).ok_or_else(|| {
            serde::de::Error::custom(format!("`{value}` is not a ListingSource label"))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceLabelError {
    #[error("the label note must not be blank")]
    BlankNote,
    #[error("the label note must be at most {MAX_SOURCE_LABEL_NOTE_CHARS} characters")]
    NoteTooLong,
    #[error("the label OTHER needs a note")]
    OtherNeedsNote,
}

/// A free-text note on a label: trimmed, not blank, at most [`MAX_SOURCE_LABEL_NOTE_CHARS`]
/// characters.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct SourceLabelNote(String);

impl SourceLabelNote {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SourceLabelNote {
    type Error = SourceLabelError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(SourceLabelError::BlankNote);
        }
        if trimmed.chars().count() > MAX_SOURCE_LABEL_NOTE_CHARS {
            return Err(SourceLabelError::NoteTooLong);
        }
        Ok(Self(trimmed.to_owned()))
    }
}

/// What a human labels a ListingSource with: a label, with a note that `OTHER` requires.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "SourceLabelPayload")]
pub struct SourceLabelRequest {
    label: SourceLabel,
    note: Option<SourceLabelNote>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceLabelPayload {
    label: SourceLabel,
    #[serde(default)]
    note: Option<SourceLabelNote>,
}

impl SourceLabelRequest {
    pub fn new(
        label: SourceLabel,
        note: Option<SourceLabelNote>,
    ) -> Result<Self, SourceLabelError> {
        if label == SourceLabel::Other && note.is_none() {
            return Err(SourceLabelError::OtherNeedsNote);
        }
        Ok(Self { label, note })
    }

    pub fn label(&self) -> SourceLabel {
        self.label
    }

    pub fn note(&self) -> Option<&SourceLabelNote> {
        self.note.as_ref()
    }
}

impl TryFrom<SourceLabelPayload> for SourceLabelRequest {
    type Error = SourceLabelError;

    fn try_from(payload: SourceLabelPayload) -> Result<Self, Self::Error> {
        Self::new(payload.label, payload.note)
    }
}

/// The label a ListingSource carries, with who set it and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppliedSourceLabel {
    pub label: SourceLabel,
    pub note: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub labelled_at: OffsetDateTime,
    pub labelled_by: ReviewActor,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_round_trip_every_label_and_refuse_other_spellings() {
        for label in SourceLabel::ALL {
            assert_eq!(Some(label), SourceLabel::from_persisted(label.as_str()));
        }
        for name in ["", "not_an_eshop", "Other", " OTHER", "UNSCRAPABLE"] {
            assert_eq!(None, SourceLabel::from_persisted(name), "{name:?}");
        }
    }

    #[test]
    fn should_trim_the_note_and_refuse_blank_and_overlong_notes() {
        let note = SourceLabelNote::try_from("  gallery, enquiry only \n".to_owned())
            .unwrap_or_else(|error| panic!("valid note: {error}"));
        assert_eq!("gallery, enquiry only", note.as_str());
        assert_eq!(
            Err(SourceLabelError::BlankNote),
            SourceLabelNote::try_from(" \t".to_owned())
        );
        assert!(SourceLabelNote::try_from("ä".repeat(MAX_SOURCE_LABEL_NOTE_CHARS)).is_ok());
        assert_eq!(
            Err(SourceLabelError::NoteTooLong),
            SourceLabelNote::try_from("ä".repeat(MAX_SOURCE_LABEL_NOTE_CHARS + 1))
        );
    }

    #[test]
    fn should_require_a_note_for_the_label_other_only() {
        assert_eq!(
            Err(SourceLabelError::OtherNeedsNote),
            SourceLabelRequest::new(SourceLabel::Other, None)
        );
        assert!(SourceLabelRequest::new(SourceLabel::NotAnEshop, None).is_ok());
    }

    #[test]
    fn should_deserialize_a_request_through_its_constructors() {
        let request: SourceLabelRequest =
            serde_json::from_str(r#"{"label":"OTHER","note":" odd shop "}"#)
                .unwrap_or_else(|error| panic!("valid request: {error}"));
        assert_eq!(SourceLabel::Other, request.label());
        assert_eq!(
            Some("odd shop"),
            request.note().map(SourceLabelNote::as_str)
        );

        for body in [
            r#"{"label":"OTHER"}"#,
            r#"{"label":"OTHER","note":"  "}"#,
            r#"{"label":"nope"}"#,
            r#"{"label":"NOT_AN_ESHOP","extra":1}"#,
            r#"{}"#,
        ] {
            assert!(
                serde_json::from_str::<SourceLabelRequest>(body).is_err(),
                "{body}"
            );
        }
    }

    #[test]
    fn should_serialize_an_applied_label_with_its_persisted_names() {
        let applied = AppliedSourceLabel {
            label: SourceLabel::ApproachNotApplicable,
            note: None,
            labelled_at: datetime!(2026-11-12 09:00:00 UTC),
            labelled_by: ReviewActor::Human,
        };

        assert_eq!(
            serde_json::json!({
                "label": "APPROACH_NOT_APPLICABLE",
                "note": null,
                "labelled_at": "2026-11-12T09:00:00Z",
                "labelled_by": "HUMAN"
            }),
            serde_json::to_value(applied).unwrap_or_else(|error| panic!("serialize: {error}"))
        );
    }
}
