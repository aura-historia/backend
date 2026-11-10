//! The operator's label that a ListingSource cannot be scraped: a closed reason and a short note,
//! set by a human in the console. Marking switches crawling off and the scope sync never switches
//! it back on (`docs/crawler/README.md#listingsource-scope`).

use serde::{Deserialize, Serialize, Serializer};

/// The longest note, in characters.
pub const MAX_UNSCRAPABLE_NOTE_CHARS: usize = 500;

/// Why a ListingSource cannot be scraped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnscrapableReason {
    BotProtection,
    JavascriptRendered,
    LoginRequired,
    NoProductPages,
    /// Any other cause; the note names it.
    Other,
}

impl UnscrapableReason {
    pub const ALL: [Self; 5] = [
        Self::BotProtection,
        Self::JavascriptRendered,
        Self::LoginRequired,
        Self::NoProductPages,
        Self::Other,
    ];

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BotProtection => "BOT_PROTECTION",
            Self::JavascriptRendered => "JAVASCRIPT_RENDERED",
            Self::LoginRequired => "LOGIN_REQUIRED",
            Self::NoProductPages => "NO_PRODUCT_PAGES",
            Self::Other => "OTHER",
        }
    }

    /// The reason a persisted or requested name stands for, or `None` for a name no reason has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

impl Serialize for UnscrapableReason {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for UnscrapableReason {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_persisted(&value).ok_or_else(|| {
            serde::de::Error::custom(format!("`{value}` is not an unscrapable reason"))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnscrapableError {
    #[error("the unscrapable note must not be blank")]
    BlankNote,
    #[error("the unscrapable note must be at most {MAX_UNSCRAPABLE_NOTE_CHARS} characters")]
    NoteTooLong,
    #[error("the reason OTHER needs a note")]
    OtherNeedsNote,
}

/// A free-text note on an unscrapable mark: trimmed, not blank, at most
/// [`MAX_UNSCRAPABLE_NOTE_CHARS`] characters.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct UnscrapableNote(String);

impl UnscrapableNote {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UnscrapableNote {
    type Error = UnscrapableError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(UnscrapableError::BlankNote);
        }
        if trimmed.chars().count() > MAX_UNSCRAPABLE_NOTE_CHARS {
            return Err(UnscrapableError::NoteTooLong);
        }
        Ok(Self(trimmed.to_owned()))
    }
}

/// What a human marks a ListingSource as: a reason, with a note that `OTHER` requires.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "UnscrapableMarkPayload")]
pub struct UnscrapableMark {
    reason: UnscrapableReason,
    note: Option<UnscrapableNote>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnscrapableMarkPayload {
    reason: UnscrapableReason,
    #[serde(default)]
    note: Option<UnscrapableNote>,
}

impl UnscrapableMark {
    pub fn new(
        reason: UnscrapableReason,
        note: Option<UnscrapableNote>,
    ) -> Result<Self, UnscrapableError> {
        if reason == UnscrapableReason::Other && note.is_none() {
            return Err(UnscrapableError::OtherNeedsNote);
        }
        Ok(Self { reason, note })
    }

    pub fn reason(&self) -> UnscrapableReason {
        self.reason
    }

    pub fn note(&self) -> Option<&UnscrapableNote> {
        self.note.as_ref()
    }
}

impl TryFrom<UnscrapableMarkPayload> for UnscrapableMark {
    type Error = UnscrapableError;

    fn try_from(payload: UnscrapableMarkPayload) -> Result<Self, Self::Error> {
        Self::new(payload.reason, payload.note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_round_trip_every_reason_and_refuse_other_spellings() {
        for reason in UnscrapableReason::ALL {
            assert_eq!(
                Some(reason),
                UnscrapableReason::from_persisted(reason.as_str())
            );
        }
        for name in ["", "bot_protection", "Other", " OTHER", "CAPTCHA"] {
            assert_eq!(None, UnscrapableReason::from_persisted(name), "{name:?}");
        }
    }

    #[test]
    fn should_trim_the_note_and_refuse_blank_and_overlong_notes() {
        let note = UnscrapableNote::try_from("  needs a login \n".to_owned()).expect("note");
        assert_eq!("needs a login", note.as_str());
        assert_eq!(
            Err(UnscrapableError::BlankNote),
            UnscrapableNote::try_from(" \t".to_owned())
        );
        assert!(UnscrapableNote::try_from("ä".repeat(MAX_UNSCRAPABLE_NOTE_CHARS)).is_ok());
        assert_eq!(
            Err(UnscrapableError::NoteTooLong),
            UnscrapableNote::try_from("ä".repeat(MAX_UNSCRAPABLE_NOTE_CHARS + 1))
        );
    }

    #[test]
    fn should_require_a_note_for_the_reason_other_only() {
        assert_eq!(
            Err(UnscrapableError::OtherNeedsNote),
            UnscrapableMark::new(UnscrapableReason::Other, None)
        );
        assert!(UnscrapableMark::new(UnscrapableReason::LoginRequired, None).is_ok());
    }

    #[test]
    fn should_deserialize_a_mark_through_its_constructors() {
        let mark: UnscrapableMark =
            serde_json::from_str(r#"{"reason":"OTHER","note":" odd shop "}"#).expect("mark");
        assert_eq!(UnscrapableReason::Other, mark.reason());
        assert_eq!(Some("odd shop"), mark.note().map(UnscrapableNote::as_str));

        for body in [
            r#"{"reason":"OTHER"}"#,
            r#"{"reason":"OTHER","note":"  "}"#,
            r#"{"reason":"nope"}"#,
            r#"{"reason":"LOGIN_REQUIRED","extra":1}"#,
            r#"{}"#,
        ] {
            assert!(
                serde_json::from_str::<UnscrapableMark>(body).is_err(),
                "{body}"
            );
        }
    }
}
