use crate::{CrawlerReviewEventId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
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

/// An event of the trail as `GET /api/review-events` lists the events of all reviews: what
/// happened to which review and by whom, without the action's details, with the review's artifact
/// type and ListingSource.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewEventSummary {
    pub event_id: CrawlerReviewEventId,
    pub review_id: CrawlerReviewId,
    pub artifact_type: String,
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    pub actor: ReviewActor,
    pub action: ReviewEventAction,
    pub candidate_version: i64,
    pub notes: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

/// Who made a review mutation. The review console resolves it from the request's credential:
/// in `agent` authoring mode the author agent's generated token acts as `AgentAuthor` and the
/// reviewer agent's as `AgentReviewer`; the review token, a browser session and the `dev` binary's
/// local same-origin requests act as `Human`. `System` is the crawler itself, which approves an
/// inferred URL pattern directly in `auto` authoring mode; no request acts as it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewActor {
    Human,
    AgentAuthor,
    AgentReviewer,
    System,
}

impl ReviewActor {
    pub const ALL: [Self; 4] = [
        Self::Human,
        Self::AgentAuthor,
        Self::AgentReviewer,
        Self::System,
    ];

    /// The actors that are review agents, whose decisions the approval guard, the hourly cap and
    /// the four-eyes rule govern.
    pub const AGENTS: [Self; 2] = [Self::AgentAuthor, Self::AgentReviewer];

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "HUMAN",
            Self::AgentAuthor => "AGENT_AUTHOR",
            Self::AgentReviewer => "AGENT_REVIEWER",
            Self::System => "SYSTEM",
        }
    }

    /// The actor a persisted name stands for, or `None` for a name no actor has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|actor| actor.as_str() == value)
    }

    pub const fn is_agent(self) -> bool {
        matches!(self, Self::AgentAuthor | Self::AgentReviewer)
    }

    /// Whether the actor may change what a review approves: its candidate, schema fields,
    /// removed-page declarations and pages. The reviewer agent only judges what the author agent or
    /// a human staged.
    pub const fn may_change_candidates(self) -> bool {
        !matches!(self, Self::AgentReviewer)
    }

    /// Whether the actor may approve or reject a review. The author agent stages candidates for
    /// the reviewer agent or a human to decide.
    pub const fn may_decide(self) -> bool {
        !matches!(self, Self::AgentAuthor)
    }

    /// Whether the actor may change a review that is no longer pending, such as the approved
    /// review whose candidate is live, where an edit rewrites the live configuration. Agents
    /// change only pending reviews, so they change live configuration only through an approval.
    pub const fn may_change_decided_reviews(self) -> bool {
        !self.is_agent()
    }

    /// Whether the actor may undo a decision, such as restoring the live configuration an approval
    /// replaced: only a human undoes a decision.
    pub const fn may_undo_decisions(self) -> bool {
        matches!(self, Self::Human)
    }
}

/// What a review mutation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEventAction {
    /// `POST /api/reviews/{id}/candidate`, which also declares removed pages, and
    /// `POST /api/reviews/{id}/sample-labels`: sample labels belong to the candidate.
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
    /// `POST /api/reviews/{id}/restore`: a human brought back what the review's approval replaced.
    Restored,
}

impl ReviewEventAction {
    pub const ALL: [Self; 8] = [
        Self::CandidateEdited,
        Self::SchemaFieldEdited,
        Self::PageAdded,
        Self::PageRemoved,
        Self::RepairRequested,
        Self::Approved,
        Self::Rejected,
        Self::Restored,
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
            Self::Restored => "RESTORED",
        }
    }

    /// The action a persisted name stands for, or `None` for a name no action has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == value)
    }

    /// The actions that change what a review approves: its candidate, removed-page declarations
    /// included, and its pages. An agent that made one of them on a review never approves it.
    pub const CHANGES_CANDIDATE: [Self; 4] = [
        Self::CandidateEdited,
        Self::SchemaFieldEdited,
        Self::PageAdded,
        Self::PageRemoved,
    ];
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
        for unknown in ["human", "Agent", "AGENT", "", "ROBOT", "AGENT_OBSERVER"] {
            assert_eq!(ReviewActor::from_persisted(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn should_let_only_a_human_undo_a_decision() {
        for actor in ReviewActor::ALL {
            assert_eq!(
                actor.may_undo_decisions(),
                actor == ReviewActor::Human,
                "{actor}"
            );
        }
    }

    #[test]
    fn should_let_no_agent_change_anything_but_pending_reviews() {
        for actor in ReviewActor::ALL {
            assert_eq!(
                actor.may_change_decided_reviews(),
                !ReviewActor::AGENTS.contains(&actor),
                "{actor}"
            );
        }
    }

    /// The author agent stages and the reviewer agent decides, so neither does the other's part;
    /// humans and the crawler do both.
    #[rstest::rstest]
    #[case::human(ReviewActor::Human, true, true)]
    #[case::author(ReviewActor::AgentAuthor, true, false)]
    #[case::reviewer(ReviewActor::AgentReviewer, false, true)]
    #[case::system(ReviewActor::System, true, true)]
    fn should_split_staging_and_deciding_between_the_agent_roles(
        #[case] actor: ReviewActor,
        #[case] changes_candidates: bool,
        #[case] decides: bool,
    ) {
        assert_eq!(actor.may_change_candidates(), changes_candidates);
        assert_eq!(actor.may_decide(), decides);
        assert_eq!(actor.is_agent(), ReviewActor::AGENTS.contains(&actor));
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
        for unknown in ["approved", "CandidateEdited", "", "RESTORE"] {
            assert_eq!(
                ReviewEventAction::from_persisted(unknown),
                None,
                "{unknown}"
            );
        }
    }
}
