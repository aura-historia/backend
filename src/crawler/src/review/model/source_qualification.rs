//! Whether a ListingSource is a real art and antiques e-shop, decided before the crawler crawls it:
//! it sells art and antiques, antique and vintage collectibles included, and sells them online. In
//! `agent` mode the author agent decides it from the shop's own pages; a human may override or
//! clear it. The contract is in `docs/crawler/README.md#listingsource-qualification`.

use super::{ReviewActor, SourceLabel, SourceLabelError, SourceLabelNote};
use listing_source_core::ListingSourceId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// What a qualification decided. Serialized and persisted by its `SCREAMING_SNAKE_CASE` name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Qualification {
    /// A real art and antiques e-shop. With a label, one the crawler cannot crawl for the label's
    /// reason.
    Qualified,
    /// Not an art and antiques e-shop, or one the crawler may not or cannot reach at all; its label
    /// says which.
    Disqualified,
    /// The decider could not tell from what it could read. A review agent's is tried again on the
    /// [park schedule](super::ParkSchedule); a human may decide it any time.
    NeedsHuman,
}

impl Qualification {
    pub const ALL: [Self; 3] = [Self::Qualified, Self::Disqualified, Self::NeedsHuman];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Qualified => "QUALIFIED",
            Self::Disqualified => "DISQUALIFIED",
            Self::NeedsHuman => "NEEDS_HUMAN",
        }
    }

    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|qualification| qualification.as_str() == value)
    }
}

impl SourceLabel {
    /// Whether the label says the source is not an art and antiques e-shop the crawler may reach at
    /// all, which a qualification decides, rather than why a qualified shop cannot be crawled.
    pub const fn disqualifies(self) -> bool {
        matches!(
            self,
            Self::NotAnEshop | Self::NotAnAntiqueShop | Self::RobotsDisallowed | Self::ShopClosed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QualificationDecisionError {
    #[error(transparent)]
    Note(#[from] SourceLabelError),
    #[error(
        "a DISQUALIFIED decision names its label: NOT_AN_ESHOP, NOT_AN_ANTIQUE_SHOP, ROBOTS_DISALLOWED or SHOP_CLOSED"
    )]
    DisqualificationNeedsItsLabel,
    #[error(
        "a QUALIFIED decision may carry only a label that keeps a real art and antiques e-shop from being crawled, not {0}"
    )]
    QualifiedWithDisqualifyingLabel(&'static str),
    #[error("a NEEDS_HUMAN decision carries no label")]
    NeedsHumanWithLabel,
    #[error("the label {0} is retired")]
    RetiredLabel(&'static str),
}

/// A qualification as decided: what it decided, the label it sets, if any, and the note with the
/// evidence, which every decision carries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "QualificationDecisionPayload")]
pub struct QualificationDecision {
    qualification: Qualification,
    label: Option<SourceLabel>,
    note: SourceLabelNote,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QualificationDecisionPayload {
    qualification: Qualification,
    #[serde(default)]
    label: Option<SourceLabel>,
    note: String,
}

impl QualificationDecision {
    pub fn new(
        qualification: Qualification,
        label: Option<SourceLabel>,
        note: SourceLabelNote,
    ) -> Result<Self, QualificationDecisionError> {
        if let Some(label) = label.filter(|label| label.is_retired()) {
            return Err(QualificationDecisionError::RetiredLabel(label.as_str()));
        }
        match (qualification, label) {
            (Qualification::Disqualified, Some(label)) if label.disqualifies() => {}
            (Qualification::Disqualified, _) => {
                return Err(QualificationDecisionError::DisqualificationNeedsItsLabel);
            }
            (Qualification::Qualified, Some(label)) if label.disqualifies() => {
                return Err(QualificationDecisionError::QualifiedWithDisqualifyingLabel(
                    label.as_str(),
                ));
            }
            (Qualification::Qualified, _) => {}
            (Qualification::NeedsHuman, Some(_)) => {
                return Err(QualificationDecisionError::NeedsHumanWithLabel);
            }
            (Qualification::NeedsHuman, None) => {}
        }
        Ok(Self {
            qualification,
            label,
            note,
        })
    }

    /// A `QUALIFIED` decision without a label, which always fits.
    pub fn qualified(note: SourceLabelNote) -> Self {
        Self {
            qualification: Qualification::Qualified,
            label: None,
            note,
        }
    }

    pub fn qualification(&self) -> Qualification {
        self.qualification
    }

    pub fn label(&self) -> Option<SourceLabel> {
        self.label
    }

    pub fn note(&self) -> &SourceLabelNote {
        &self.note
    }
}

impl TryFrom<QualificationDecisionPayload> for QualificationDecision {
    type Error = QualificationDecisionError;

    fn try_from(payload: QualificationDecisionPayload) -> Result<Self, Self::Error> {
        Self::new(
            payload.qualification,
            payload.label,
            SourceLabelNote::try_from(payload.note)?,
        )
    }
}

/// The qualification a ListingSource carries, with who decided it and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppliedQualification {
    pub qualification: Qualification,
    pub note: String,
    #[serde(with = "time::serde::rfc3339")]
    pub qualified_at: OffsetDateTime,
    pub qualified_by: ReviewActor,
}

/// What a decided qualification did to the source's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QualificationLabelEffect {
    /// The source had no label and the decision set none.
    None,
    /// The decision set its label, replacing the label the source had, if any.
    Labelled,
    /// The decision qualified a source without a reason it cannot be crawled, but a review agent
    /// clears no label: the source keeps its label, stays disabled and waits for a human, who
    /// clears the label or leaves it.
    KeptLabel,
    /// A human qualified the source without a label, which cleared the label it had.
    ClearedLabel,
}

/// The answer to a decided qualification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QualificationOutcome {
    #[serde(flatten)]
    pub applied: AppliedQualification,
    pub label_effect: QualificationLabelEffect,
    /// The label the source carries after the decision, if any.
    pub label: Option<SourceLabel>,
    /// The label the decision replaced or cleared, or kept against it, with who had set it.
    pub previous_label: Option<PreviousSourceLabel>,
}

/// A label a qualification met on the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviousSourceLabel {
    pub label: SourceLabel,
    pub labelled_by: ReviewActor,
}

/// One entry of a ListingSource's append-only qualification history: a decision with its label
/// and note, or a clear (`qualification` `None`), with who did it and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QualificationEvent {
    pub qualification: Option<Qualification>,
    pub label: Option<SourceLabel>,
    pub note: Option<String>,
    pub actor: ReviewActor,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
}

/// A ListingSource the scope admits to crawling that waits for its qualification, as the agents'
/// qualification queue lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceAwaitingQualification {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    /// The start page the decider opens first.
    pub crawl_root_url: String,
    pub crawl_host: String,
    /// The source existed before qualification did: it is crawled while it waits, and its one
    /// qualification may replace a label a human set.
    pub backfill: bool,
    /// The label the source carries, which the decision may replace, as evidence.
    pub source_label: Option<QueuedSourceLabel>,
    /// The artifact types of the source's pending reviews, which an approach label must name.
    pub pending_review_artifacts: Vec<String>,
    /// The note of the review agent's earlier `NEEDS_HUMAN` this try follows, as evidence; `None`
    /// for a source never decided.
    pub earlier_note: Option<String>,
    /// Whether this try is the agents' final round: their daily `NEEDS_HUMAN` tries since a human
    /// last decided are spent, so the strongest model decides, and a further `NEEDS_HUMAN` parks
    /// the source.
    pub final_round: bool,
}

/// A label a queued source carries, with its note, as the decider reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueuedSourceLabel {
    pub label: SourceLabel,
    pub note: Option<String>,
    pub labelled_by: ReviewActor,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(text: &str) -> SourceLabelNote {
        SourceLabelNote::try_from(text.to_owned()).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn should_round_trip_every_qualification_by_its_persisted_name() {
        for qualification in Qualification::ALL {
            assert_eq!(
                Some(qualification),
                Qualification::from_persisted(qualification.as_str())
            );
            assert_eq!(
                serde_json::json!(qualification.as_str()),
                serde_json::to_value(qualification).unwrap_or_else(|error| panic!("{error}"))
            );
        }
        assert_eq!(None, Qualification::from_persisted("qualified"));
    }

    #[test]
    fn should_tell_the_labels_a_qualification_decides_from_the_others() {
        let disqualifying = SourceLabel::ALL
            .into_iter()
            .filter(|label| label.disqualifies())
            .collect::<Vec<_>>();
        assert_eq!(
            vec![
                SourceLabel::NotAnEshop,
                SourceLabel::NotAnAntiqueShop,
                SourceLabel::RobotsDisallowed,
                SourceLabel::ShopClosed
            ],
            disqualifying
        );
    }

    #[test]
    fn should_accept_only_a_label_that_fits_the_decision() {
        let accepted = [
            (Qualification::Qualified, None),
            (Qualification::Qualified, Some(SourceLabel::BotProtection)),
            (
                Qualification::Qualified,
                Some(SourceLabel::UrlPatternApproachNotApplicable),
            ),
            (Qualification::Disqualified, Some(SourceLabel::NotAnEshop)),
            (Qualification::Disqualified, Some(SourceLabel::ShopClosed)),
            (Qualification::NeedsHuman, None),
        ];
        for (qualification, label) in accepted {
            assert!(
                QualificationDecision::new(qualification, label, note("seen")).is_ok(),
                "{qualification:?} {label:?}"
            );
        }
        let refused = [
            (
                Qualification::Disqualified,
                None,
                QualificationDecisionError::DisqualificationNeedsItsLabel,
            ),
            (
                Qualification::Disqualified,
                Some(SourceLabel::BotProtection),
                QualificationDecisionError::DisqualificationNeedsItsLabel,
            ),
            (
                Qualification::Qualified,
                Some(SourceLabel::NotAnAntiqueShop),
                QualificationDecisionError::QualifiedWithDisqualifyingLabel("NOT_AN_ANTIQUE_SHOP"),
            ),
            (
                Qualification::NeedsHuman,
                Some(SourceLabel::Other),
                QualificationDecisionError::NeedsHumanWithLabel,
            ),
            (
                Qualification::Qualified,
                Some(SourceLabel::ApproachNotApplicable),
                QualificationDecisionError::RetiredLabel("APPROACH_NOT_APPLICABLE"),
            ),
        ];
        for (qualification, label, error) in refused {
            assert_eq!(
                Err(error),
                QualificationDecision::new(qualification, label, note("seen"))
            );
        }
    }

    #[test]
    fn should_deserialize_a_decision_through_its_constructors_and_require_a_note() {
        let decision: QualificationDecision = serde_json::from_str(
            r#"{"qualification":"DISQUALIFIED","label":"NOT_AN_ANTIQUE_SHOP","note":" new furniture only "}"#,
        )
        .unwrap_or_else(|error| panic!("valid decision: {error}"));
        assert_eq!(Qualification::Disqualified, decision.qualification());
        assert_eq!(Some(SourceLabel::NotAnAntiqueShop), decision.label());
        assert_eq!("new furniture only", decision.note().as_str());

        for body in [
            r#"{"qualification":"QUALIFIED"}"#,
            r#"{"qualification":"QUALIFIED","note":"  "}"#,
            r#"{"qualification":"DISQUALIFIED","note":"no label"}"#,
            r#"{"qualification":"QUALIFIED","note":"x","extra":1}"#,
            r#"{"qualification":"qualified","note":"x"}"#,
        ] {
            assert!(
                serde_json::from_str::<QualificationDecision>(body).is_err(),
                "{body}"
            );
        }
    }
}
