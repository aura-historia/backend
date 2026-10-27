//! The one crawler setting that decides how URL patterns and product schemas are authored.

/// How crawler configuration artifacts (URL patterns and product schemas) come into being.
///
/// `CRAWLER_AUTHORING_MODE` selects it for the whole crawler; the operator contract is in
/// `docs/crawler/operations.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrawlerAuthoringMode {
    /// The LLM generates artifacts. An inferred URL pattern is applied directly; a generated
    /// schema is approved without review when deterministic coverage passes and the LLM
    /// evaluation is a high-confidence approval, otherwise it waits for human review.
    Auto,
    /// The LLM generates artifacts and every one waits for human review.
    #[default]
    Review,
    /// No LLM call: where generation would run, the crawler opens a review an operator
    /// completes by hand.
    Manual,
    /// As `manual` for the crawler, which makes no LLM call and opens the same reviews; the author
    /// agent stages URL patterns and schemas through the review console and the reviewer agent
    /// approves or rejects them within the approval guard, each with a token generated at start.
    Agent,
}

impl CrawlerAuthoringMode {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Review, Self::Manual, Self::Agent];

    pub fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == normalized)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Review => "review",
            Self::Manual => "manual",
            Self::Agent => "agent",
        }
    }

    /// Whether the mode makes LLM calls at all. Without them, where generation would run, the
    /// crawler opens a review for an operator or the review agents to complete.
    pub const fn uses_llm(self) -> bool {
        !matches!(self, Self::Manual | Self::Agent)
    }

    /// Whether the review agents work the review console: the author agent stages candidates and
    /// the reviewer agent decides them.
    pub const fn has_review_agents(self) -> bool {
        matches!(self, Self::Agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_every_mode_case_insensitively_with_surrounding_whitespace() {
        assert_eq!(
            CrawlerAuthoringMode::parse("auto"),
            Some(CrawlerAuthoringMode::Auto)
        );
        assert_eq!(
            CrawlerAuthoringMode::parse(" Review "),
            Some(CrawlerAuthoringMode::Review)
        );
        assert_eq!(
            CrawlerAuthoringMode::parse("MANUAL"),
            Some(CrawlerAuthoringMode::Manual)
        );
        assert_eq!(
            CrawlerAuthoringMode::parse("Agent"),
            Some(CrawlerAuthoringMode::Agent)
        );
    }

    #[test]
    fn should_reject_removed_and_unknown_values() {
        for value in [
            "report_only",
            "human_only",
            "auto_approve_high_confidence",
            "",
            "jev_auto",
        ] {
            assert_eq!(CrawlerAuthoringMode::parse(value), None, "{value:?}");
        }
    }

    #[test]
    fn should_default_to_review_so_auto_approval_is_an_explicit_opt_in() {
        assert_eq!(
            CrawlerAuthoringMode::default(),
            CrawlerAuthoringMode::Review
        );
        assert!(CrawlerAuthoringMode::default().uses_llm());
        assert!(!CrawlerAuthoringMode::Manual.uses_llm());
    }

    /// `agent` is `manual` for the crawler, with the review agents on the console.
    #[test]
    fn should_make_no_llm_call_and_run_the_review_agents_only_in_agent_mode() {
        for mode in CrawlerAuthoringMode::ALL {
            assert_eq!(
                mode.has_review_agents(),
                mode == CrawlerAuthoringMode::Agent,
                "{mode:?}"
            );
        }
        assert!(!CrawlerAuthoringMode::Agent.uses_llm());
    }
}
