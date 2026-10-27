use crate::{CrawlerReviewEventId, CrawlerReviewId};
use serde::{Serialize, Serializer};
use time::OffsetDateTime;

/// One accepted mutation of a review in its append-only event trail: who made it, what it did
/// and the review's candidate version after it.
#[derive(Debug, Clone, Serialize)]
pub struct CrawlerReviewEvent {
    pub event_id: CrawlerReviewEventId,
    pub review_id: CrawlerReviewId,
    pub actor: ReviewActor,
    pub action: ReviewEventAction,
    pub candidate_version: i64,
    /// The notes of a verdict: the needs-repair note, or the approval or rejection notes.
    pub notes: Option<String>,
    /// What the action keeps, such as the edited field, the page, or what an approval replaced
    /// in the ListingSource's live configuration.
    pub details: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

/// Who made a review mutation. The review console resolves it from the request's credential:
/// the agent token acts as `Agent`; the review token, a browser session and the `dev` binary's
/// local same-origin requests act as `Human`. `System` is the crawler itself, which approves an
/// inferred URL pattern directly in `auto` authoring mode; no request acts as it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewActor {
    Human,
    Agent,
    System,
}

impl ReviewActor {
    pub const ALL: [Self; 3] = [Self::Human, Self::Agent, Self::System];

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "HUMAN",
            Self::Agent => "AGENT",
            Self::System => "SYSTEM",
        }
    }

    /// The actor a persisted name stands for, or `None` for a name no actor has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|actor| actor.as_str() == value)
    }

    /// Whether the actor may change a review that is no longer pending, such as the approved
    /// review whose candidate is live, where an edit rewrites the live configuration. The agent
    /// changes only pending reviews, so it changes live configuration only through an approval.
    pub const fn may_change_decided_reviews(self) -> bool {
        !matches!(self, Self::Agent)
    }
}

/// What a review mutation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEventAction {
    /// `POST /api/reviews/{id}/candidate`, which also declares removed pages.
    CandidateEdited,
    /// `POST /api/reviews/{id}/schema-field`.
    SchemaFieldEdited,
    /// `POST /api/reviews/{id}/pages`.
    PageAdded,
    /// `DELETE /api/review-pages/{id}`.
    PageRemoved,
    RepairRequested,
    Approved,
    Rejected,
}

impl ReviewEventAction {
    pub const ALL: [Self; 7] = [
        Self::CandidateEdited,
        Self::SchemaFieldEdited,
        Self::PageAdded,
        Self::PageRemoved,
        Self::RepairRequested,
        Self::Approved,
        Self::Rejected,
    ];

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CandidateEdited => "CANDIDATE_EDITED",
            Self::SchemaFieldEdited => "SCHEMA_FIELD_EDITED",
            Self::PageAdded => "PAGE_ADDED",
            Self::PageRemoved => "PAGE_REMOVED",
            Self::RepairRequested => "REPAIR_REQUESTED",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }

    /// The action a persisted name stands for, or `None` for a name no action has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == value)
    }
}

impl std::fmt::Display for ReviewActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ReviewActor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for ReviewEventAction {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_read_back_every_persisted_actor_and_refuse_any_other_name() {
        for actor in ReviewActor::ALL {
            assert_eq!(ReviewActor::from_persisted(actor.as_str()), Some(actor));
            assert_eq!(
                serde_json::to_value(actor).ok(),
                Some(serde_json::Value::from(actor.as_str()))
            );
        }
        for unknown in ["human", "Agent", "", "ROBOT"] {
            assert_eq!(ReviewActor::from_persisted(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn should_let_only_the_agent_change_nothing_but_pending_reviews() {
        assert!(!ReviewActor::Agent.may_change_decided_reviews());
        assert!(ReviewActor::Human.may_change_decided_reviews());
        assert!(ReviewActor::System.may_change_decided_reviews());
    }

    #[test]
    fn should_read_back_every_persisted_action_and_refuse_any_other_name() {
        for action in ReviewEventAction::ALL {
            assert_eq!(
                ReviewEventAction::from_persisted(action.as_str()),
                Some(action)
            );
            assert_eq!(
                serde_json::to_value(action).ok(),
                Some(serde_json::Value::from(action.as_str()))
            );
        }
        for unknown in ["approved", "CandidateEdited", "", "RESTORED"] {
            assert_eq!(
                ReviewEventAction::from_persisted(unknown),
                None,
                "{unknown}"
            );
        }
    }
}
