//! The label of a ListingSource the crawler should not crawl: a site that is not an antique
//! e-shop, one the crawler's approach cannot scrape, or one kept out of reach. Labelling switches
//! crawling off and the scope sync keeps it off until a human clears the label
//! (`docs/crawler/README.md#listingsource-scope`).

use super::{ArtifactType, ReviewActor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The longest label note, in characters.
pub const MAX_SOURCE_LABEL_NOTE_CHARS: usize = 500;

/// Why a ListingSource is not crawled. Serialized, persisted and offered to the agents by its
/// `SCREAMING_SNAKE_CASE` name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceLabel {
    /// The site offers no items for sale, such as a brochure site or an artist's portfolio. A
    /// catalogue of items that can be bought or enquired about, each showing a price, price on
    /// request, an estimate or an availability status, is an e-shop: enquiry-only stock, an
    /// auction's lots and a marketplace's items alike.
    NotAnEshop,
    /// The site offers items for sale, but not art or antiques; antique and vintage collectibles count as
    /// antiques, ordinary second-hand goods, new collectibles and reproductions do not.
    NotAnAntiqueShop,
    /// The shop's product URLs cannot be told apart by a URL pattern.
    UrlPatternApproachNotApplicable,
    /// No CSS or JSON-LD product schema extracts the shop's product pages reliably.
    ProductSchemaApproachNotApplicable,
    /// The shop's robots.txt refuses the crawler: a refused permission, not a technical block.
    RobotsDisallowed,
    /// The domain is parked or for sale, the business shut down, or the site is long gone.
    ShopClosed,
    /// A bot protection keeps the crawler out.
    BotProtection,
    /// The product pages are rendered by JavaScript the crawler does not run.
    JavascriptRendered,
    /// The products are behind a login.
    LoginRequired,
    /// Any other cause; the note names it.
    Other,
    /// Retired: the shop sells online, but one of the crawler's approaches does not fit it. It
    /// does not say which, so existing labels stay readable until the label is replaced, and no
    /// new label may use it.
    #[schemars(skip)]
    ApproachNotApplicable,
}

impl SourceLabel {
    /// Every label a ListingSource may carry, the retired one included.
    pub const ALL: [Self; 11] = [
        Self::NotAnEshop,
        Self::NotAnAntiqueShop,
        Self::UrlPatternApproachNotApplicable,
        Self::ProductSchemaApproachNotApplicable,
        Self::RobotsDisallowed,
        Self::ShopClosed,
        Self::BotProtection,
        Self::JavascriptRendered,
        Self::LoginRequired,
        Self::Other,
        Self::ApproachNotApplicable,
    ];

    /// The labels a new label may use: every label but the retired one.
    pub const OFFERED: [Self; 10] = [
        Self::NotAnEshop,
        Self::NotAnAntiqueShop,
        Self::UrlPatternApproachNotApplicable,
        Self::ProductSchemaApproachNotApplicable,
        Self::RobotsDisallowed,
        Self::ShopClosed,
        Self::BotProtection,
        Self::JavascriptRendered,
        Self::LoginRequired,
        Self::Other,
    ];

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotAnEshop => "NOT_AN_ESHOP",
            Self::NotAnAntiqueShop => "NOT_AN_ANTIQUE_SHOP",
            Self::UrlPatternApproachNotApplicable => "URL_PATTERN_APPROACH_NOT_APPLICABLE",
            Self::ProductSchemaApproachNotApplicable => "PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE",
            Self::RobotsDisallowed => "ROBOTS_DISALLOWED",
            Self::ShopClosed => "SHOP_CLOSED",
            Self::BotProtection => "BOT_PROTECTION",
            Self::JavascriptRendered => "JAVASCRIPT_RENDERED",
            Self::LoginRequired => "LOGIN_REQUIRED",
            Self::Other => "OTHER",
            Self::ApproachNotApplicable => "APPROACH_NOT_APPLICABLE",
        }
    }

    pub const fn is_retired(self) -> bool {
        matches!(self, Self::ApproachNotApplicable)
    }

    /// The review artifact a label set on a review must belong to: an approach label names the
    /// approach of the review it was decided on; any other label fits every review.
    pub const fn required_artifact(self) -> Option<ArtifactType> {
        match self {
            Self::UrlPatternApproachNotApplicable => Some(ArtifactType::UrlPattern),
            Self::ProductSchemaApproachNotApplicable => Some(ArtifactType::ProductSchema),
            _ => None,
        }
    }

    /// The label a persisted or requested name stands for, or `None` for a name no label has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|label| label.as_str() == value)
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
    #[error(
        "the label APPROACH_NOT_APPLICABLE is retired; use URL_PATTERN_APPROACH_NOT_APPLICABLE or PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE"
    )]
    RetiredLabel,
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

/// What a ListingSource is labelled with: a label that is not retired, with a note that `OTHER`
/// requires.
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
        if label.is_retired() {
            return Err(SourceLabelError::RetiredLabel);
        }
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

impl super::QueryValue for SourceLabel {
    /// Every label a source may carry, the retired one included, so the console finds the sources
    /// that still carry it.
    const VALUES: &'static [(&'static str, Self)] = &[
        ("NOT_AN_ESHOP", Self::NotAnEshop),
        ("NOT_AN_ANTIQUE_SHOP", Self::NotAnAntiqueShop),
        (
            "URL_PATTERN_APPROACH_NOT_APPLICABLE",
            Self::UrlPatternApproachNotApplicable,
        ),
        (
            "PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE",
            Self::ProductSchemaApproachNotApplicable,
        ),
        ("ROBOTS_DISALLOWED", Self::RobotsDisallowed),
        ("SHOP_CLOSED", Self::ShopClosed),
        ("BOT_PROTECTION", Self::BotProtection),
        ("JAVASCRIPT_RENDERED", Self::JavascriptRendered),
        ("LOGIN_REQUIRED", Self::LoginRequired),
        ("OTHER", Self::Other),
        ("APPROACH_NOT_APPLICABLE", Self::ApproachNotApplicable),
    ];
}

/// What one entry of a ListingSource's label history did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceLabelEventKind {
    /// The source was labelled, replacing the label it had, if any.
    Labelled,
    /// The source's label was cleared.
    Cleared,
}

impl SourceLabelEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Labelled => "LABELLED",
            Self::Cleared => "CLEARED",
        }
    }

    pub fn from_persisted(value: &str) -> Option<Self> {
        [Self::Labelled, Self::Cleared]
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

/// One entry of a ListingSource's append-only label history: the label set, with its note, or a
/// clear, with who did it and when. The label an entry replaced is the entry before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLabelEvent {
    pub event: SourceLabelEventKind,
    pub label: Option<SourceLabel>,
    pub note: Option<String>,
    pub actor: ReviewActor,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_round_trip_every_label_and_refuse_other_spellings() {
        for label in SourceLabel::ALL {
            assert_eq!(Some(label), SourceLabel::from_persisted(label.as_str()));
            assert_eq!(
                serde_json::json!(label.as_str()),
                serde_json::to_value(label).unwrap_or_else(|error| panic!("{error}")),
                "the serialized name is the persisted one"
            );
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
    fn should_offer_every_label_but_the_retired_one_and_refuse_it_for_a_new_label() {
        assert_eq!(
            SourceLabel::ALL
                .into_iter()
                .filter(|label| !label.is_retired())
                .collect::<Vec<_>>(),
            SourceLabel::OFFERED.to_vec()
        );
        assert_eq!(
            Err(SourceLabelError::RetiredLabel),
            SourceLabelRequest::new(SourceLabel::ApproachNotApplicable, None)
        );
        assert!(
            serde_json::from_str::<SourceLabelRequest>(
                r#"{"label":"APPROACH_NOT_APPLICABLE","note":"old"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn should_tie_each_approach_label_to_its_review_artifact_only() {
        assert_eq!(
            Some(ArtifactType::UrlPattern),
            SourceLabel::UrlPatternApproachNotApplicable.required_artifact()
        );
        assert_eq!(
            Some(ArtifactType::ProductSchema),
            SourceLabel::ProductSchemaApproachNotApplicable.required_artifact()
        );
        for label in SourceLabel::OFFERED.into_iter().filter(|label| {
            !matches!(
                label,
                SourceLabel::UrlPatternApproachNotApplicable
                    | SourceLabel::ProductSchemaApproachNotApplicable
            )
        }) {
            assert_eq!(None, label.required_artifact(), "{label:?}");
        }
    }

    #[test]
    fn should_name_every_label_in_a_query_by_its_persisted_name() {
        use crate::review::model::QueryValue;
        assert_eq!(
            SourceLabel::VALUES
                .iter()
                .map(|(name, label)| (*name, *label))
                .collect::<Vec<_>>(),
            SourceLabel::ALL
                .iter()
                .map(|label| (label.as_str(), *label))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn should_leave_the_retired_label_out_of_the_agents_schema() {
        let schema = serde_json::to_value(schemars::schema_for!(SourceLabel))
            .unwrap_or_else(|error| panic!("schema: {error}"));
        let offered = schema.to_string();
        for label in SourceLabel::OFFERED {
            assert!(offered.contains(label.as_str()), "{label:?}");
        }
        assert!(
            !offered.contains("\"APPROACH_NOT_APPLICABLE\""),
            "{offered}"
        );
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
