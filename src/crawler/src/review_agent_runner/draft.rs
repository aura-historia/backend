//! The structured-data draft step of a first-round product-schema review: before any author
//! process, the runner asks the console to stage its draft of the review's pages (`POST
//! /api/reviews/{id}/draft`), refreshes the matrix and reads the approval guard's checks. A
//! complete draft the guard accepts goes to the reviewer without an author; otherwise the author
//! starts from the staged draft and the fields and checks the step names. The runner decides
//! nothing about the draft itself: the console drafts and guards, the reviewer judges
//! (`docs/crawler/operations.md`, review agent runner).

use super::queue::approval_failures;
use crate::CrawlerReviewId;
use crate::review_mcp::{ConsoleClient, ConsoleError};
use serde::Deserialize;
use std::time::Duration;

/// How long the step waits for a matrix refresh the console runs in the background.
const MATRIX_WAIT: Duration = Duration::from_secs(180);
const MATRIX_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// What the draft step left on a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DraftOutcome {
    /// The fields the pages need that the draft has no rule for, as the console names them.
    pub(super) undrafted: Vec<String>,
    /// The approval guard's failed checks on the refreshed matrix; `None` when they could not be
    /// read, such as when the refresh did not finish in time.
    pub(super) failed_checks: Option<Vec<String>>,
    /// The sold page the draft added as a review page.
    pub(super) sold_page_added: Option<String>,
    /// The ListingSource URLs the draft's sold-page search fetched.
    pub(super) sold_page_candidates: Vec<String>,
}

impl DraftOutcome {
    /// Whether the draft can go to the reviewer without an author: it drafted every field the
    /// pages need and the guard accepts it on a fresh matrix.
    pub(super) fn is_ready_for_reviewer(&self) -> bool {
        self.undrafted.is_empty() && self.failed_checks.as_ref().is_some_and(Vec::is_empty)
    }

    /// The author's starting point, appended to its prompt: the field and check names the console
    /// gives and the stored product URLs its sold-page search fetched, no page content, which the
    /// author reads through its tools as data.
    pub(super) fn author_note(&self) -> String {
        let mut note = String::from(
            "\n\nA structured-data draft of this review's pages is staged as its candidate. Start from it: change only what it lacks or gets wrong.",
        );
        if !self.undrafted.is_empty() {
            note.push_str(&format!(
                " Fields the draft has no rule for: {}.",
                self.undrafted.join(", ")
            ));
        }
        match &self.failed_checks {
            Some(failed) if !failed.is_empty() => note.push_str(&format!(
                " The approval guard refuses it with: {}; check_approval names the pages.",
                failed.join(", ")
            )),
            Some(_) => {}
            None => note.push_str(" Its matrix is not refreshed yet: refresh_matrix first."),
        }
        if self.sold_page_added.is_some() {
            note.push_str(" The draft added a sold page it found among the source_product_urls.");
        } else if !self.sold_page_candidates.is_empty() {
            note.push_str(&format!(
                " The draft's state rule read no sold product on these {} source_product_urls, which count as inspected for record_no_sold_page: {}. Search on the other pages and elements where a sold label could sit.",
                self.sold_page_candidates.len(),
                self.sold_page_candidates.join(", ")
            ));
        }
        note
    }
}

#[derive(Debug, Deserialize)]
struct DraftAnswer {
    drafted: bool,
    #[serde(default)]
    undrafted: Vec<String>,
    #[serde(default)]
    sold_page_added: Option<String>,
    #[serde(default)]
    sold_page_candidates: Vec<String>,
}

/// Stages the console's draft of `review_id`, refreshes its matrix and reads the guard's checks;
/// `None` when the console drafted nothing, such as on a review whose starter schema someone
/// already replaced.
pub(super) async fn stage_draft(
    console: &ConsoleClient,
    review_id: CrawlerReviewId,
) -> Result<Option<DraftOutcome>, ConsoleError> {
    let answer: DraftAnswer = console
        .post(
            &format!("/api/reviews/{review_id}/draft"),
            &serde_json::json!({}),
        )
        .await?
        .decode()?;
    if !answer.drafted {
        return Ok(None);
    }
    let failed_checks = if refresh_matrix(console, review_id).await? {
        approval_failures(console, review_id).await?
    } else {
        None
    };
    Ok(Some(DraftOutcome {
        undrafted: answer.undrafted,
        failed_checks,
        sold_page_added: answer.sold_page_added,
        sold_page_candidates: answer.sold_page_candidates,
    }))
}

/// Refreshes the review's matrix and waits for it; `false` when it is still refreshing after
/// [`MATRIX_WAIT`].
async fn refresh_matrix(
    console: &ConsoleClient,
    review_id: CrawlerReviewId,
) -> Result<bool, ConsoleError> {
    let mut answer = console
        .get(&format!("/api/reviews/{review_id}/matrix?refresh=true"))
        .await?;
    let started = tokio::time::Instant::now();
    while answer.status == 202 {
        if started.elapsed() >= MATRIX_WAIT {
            return Ok(false);
        }
        tokio::time::sleep(MATRIX_POLL_INTERVAL).await;
        answer = console
            .get(&format!("/api/reviews/{review_id}/matrix"))
            .await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(undrafted: &[&str], failed_checks: Option<&[&str]>) -> DraftOutcome {
        let names = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        DraftOutcome {
            undrafted: names(undrafted),
            failed_checks: failed_checks.map(names),
            sold_page_added: None,
            sold_page_candidates: names(&["https://shop.example/a", "https://shop.example/b"]),
        }
    }

    #[test]
    fn should_send_only_a_complete_draft_the_guard_accepts_to_the_reviewer() {
        assert!(outcome(&[], Some(&[])).is_ready_for_reviewer());
        assert!(!outcome(&["IMAGES"], Some(&[])).is_ready_for_reviewer());
        assert!(!outcome(&[], Some(&["NO_SOLD_PAGE_TESTED"])).is_ready_for_reviewer());
        assert!(!outcome(&[], None).is_ready_for_reviewer());
    }

    #[test]
    fn should_name_the_undrafted_fields_and_failed_checks_for_the_author() {
        let note = outcome(&["IMAGES"], Some(&["NO_SOLD_PAGE_TESTED"])).author_note();

        assert!(
            note.contains("Fields the draft has no rule for: IMAGES."),
            "{note}"
        );
        assert!(
            note.contains("refuses it with: NO_SOLD_PAGE_TESTED"),
            "{note}"
        );
        assert!(
            note.contains("on these 2 source_product_urls, which count as inspected for record_no_sold_page: https://shop.example/a, https://shop.example/b."),
            "{note}"
        );
    }
}
