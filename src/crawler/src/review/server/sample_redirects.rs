//! The console's fetch of a URL-pattern review's redirect probe URLs, so the approval guard can
//! see where the URLs a candidate takes lead before an agent approves it. The contract is in
//! `docs/crawler/extraction-and-review.md` (approval guard).

use super::ReviewServer;
use crate::CrawlerReviewId;
use std::time::Duration;
use tracing::{debug, warn};

/// How long a request waits for the probe fetches, leaving room within the request deadline for
/// the guard and the verdict. Fetches still due go on in the background and record their result
/// for the next read.
const REDIRECT_PROBE_WAIT: Duration = Duration::from_secs(15);
/// How long one probe fetch may take, its wait behind the shop's crawl delay included. The probe
/// fetches in the background one URL after the other, so it does not use the live-inspection
/// deadline a request has to answer within.
const REDIRECT_PROBE_FETCH_DEADLINE: Duration = Duration::from_secs(120);

impl ReviewServer {
    /// Fetches the redirect probe URLs of the review `review_id` that have no recent fetch
    /// recorded, one after the other at the shop's pacing, and records where each led. Best
    /// effort: a URL whose fetch fails records nothing, and nothing here refuses the request; the
    /// guard refuses while none of the URLs has a recorded fetch.
    pub(super) async fn probe_sample_redirects(&self, review_id: CrawlerReviewId) {
        let due = match self.repository.redirect_probes_due(review_id).await {
            Ok(due) if !due.is_empty() => due,
            Ok(_) => return,
            Err(error) => {
                warn!(review_id = %review_id, error = %error, "could not read the redirect probe URLs of the review");
                return;
            }
        };
        let server = self.clone();
        let probes = tokio::spawn(async move {
            for url in due {
                let fetched = match tokio::time::timeout(
                    REDIRECT_PROBE_FETCH_DEADLINE,
                    server.html_fetcher.fetch(&url),
                )
                .await
                {
                    Ok(Ok(fetched)) => fetched,
                    Ok(Err(error)) => {
                        debug!(review_id = %review_id, url = %url, error = %error, "could not fetch a redirect probe URL");
                        continue;
                    }
                    Err(_) => {
                        debug!(review_id = %review_id, url = %url, "a redirect probe URL's fetch timed out");
                        continue;
                    }
                };
                if let Err(error) = server
                    .repository
                    .record_sample_redirect(review_id, &url, &fetched.final_url)
                    .await
                {
                    warn!(review_id = %review_id, error = %error, "could not record where a sample URL redirected");
                }
            }
        });
        // Dropping the handle at the deadline leaves the task running.
        let _ = tokio::time::timeout(REDIRECT_PROBE_WAIT, probes).await;
    }
}
