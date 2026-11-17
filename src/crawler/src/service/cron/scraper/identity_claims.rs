//! The identities a scraper batch's first scrapes claimed before their captures are durable
//! (`docs/crawler/networking.md#duplicate-source-listing-ids`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// The URL that claimed an identity in the batch, with the title its scrape found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Claimant {
    pub(super) url: url::Url,
    pub(super) title: Option<String>,
}

/// The identities a batch's first scrapes claimed, each with the URL that claimed it. A captured
/// URL's identity reaches the database only once the raw-capture collector pushes its capture,
/// in batches, so until then the database lookup cannot see it: another spelling of the same
/// product scraped later in the batch finds the claim here instead. A claim lasts only while its
/// capture may still become durable: the [`IdentityClaim`] releases it when the claimant ends
/// without a durable capture, so a later spelling is scraped as a first scrape again rather than
/// retired as the duplicate of a URL that captured nothing. The pass waits for the collector
/// before it ends, so later passes find the identity in the database.
#[derive(Default)]
pub(super) struct ClaimedIdentities(Mutex<HashMap<String, Claimant>>);

impl ClaimedIdentities {
    /// Claims `source_listing_id` for `url`, or answers the URL that claimed it first. The URL
    /// that holds the claim may claim it again.
    pub(super) fn claim(
        self: &Arc<Self>,
        source_listing_id: &str,
        url: &url::Url,
        title: Option<&str>,
    ) -> Result<IdentityClaim, Claimant> {
        let mut claimed = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match claimed.get(source_listing_id) {
            Some(claimant) if claimant.url != *url => Err(claimant.clone()),
            Some(_) => Ok(self.claim_of(source_listing_id, url)),
            None => {
                claimed.insert(
                    source_listing_id.to_owned(),
                    Claimant {
                        url: url.clone(),
                        title: title.map(str::to_owned),
                    },
                );
                Ok(self.claim_of(source_listing_id, url))
            }
        }
    }

    /// Releases `url`'s claim of `source_listing_id`; a claim another URL holds stays.
    pub(super) fn release(&self, source_listing_id: &str, url: &url::Url) {
        let mut claimed = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if claimed
            .get(source_listing_id)
            .is_some_and(|claimant| claimant.url == *url)
        {
            claimed.remove(source_listing_id);
        }
    }

    fn claim_of(self: &Arc<Self>, source_listing_id: &str, url: &url::Url) -> IdentityClaim {
        IdentityClaim {
            claims: Arc::clone(self),
            source_listing_id: source_listing_id.to_owned(),
            url: url.clone(),
            kept: false,
        }
    }
}

/// A claim a first scrape holds until its outcome is known: [`IdentityClaim::keep`] once its
/// capture is durable keeps it for the rest of the batch; dropped without that, on any other
/// outcome (a failed or stale write, an error before the capture was queued, a closed collector),
/// it is released.
pub(super) struct IdentityClaim {
    claims: Arc<ClaimedIdentities>,
    source_listing_id: String,
    url: url::Url,
    kept: bool,
}

impl IdentityClaim {
    /// Keeps the claim: the claimant's capture is durable.
    pub(super) fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for IdentityClaim {
    fn drop(&mut self) {
        if !self.kept {
            self.claims.release(&self.source_listing_id, &self.url);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: &str = "#2798";

    fn url(path: &str) -> url::Url {
        url::Url::parse(&format!("https://example.com{path}")).unwrap()
    }

    #[test]
    fn should_answer_the_first_claimant_to_another_url() {
        let claims = Arc::new(ClaimedIdentities::default());
        let first = url("/product/223/table/");
        let second = url("/product/223/table-1/");

        let claim = claims.claim(IDENTITY, &first, Some("Table")).unwrap();
        let again = claims.claim(IDENTITY, &first, Some("Table")).unwrap();

        assert_eq!(
            claims.claim(IDENTITY, &second, None).err(),
            Some(Claimant {
                url: first,
                title: Some("Table".to_owned())
            })
        );
        assert!(claims.claim("another identity", &second, None).is_ok());
        claim.keep();
        again.keep();
    }

    /// The claimant's capture failed: a later spelling claims the identity itself instead of
    /// being retired as the duplicate of a URL that captured nothing.
    #[test]
    fn should_release_a_claim_whose_capture_did_not_become_durable() {
        let claims = Arc::new(ClaimedIdentities::default());
        let first = url("/product/223/table/");
        let second = url("/product/223/table-1/");

        drop(claims.claim(IDENTITY, &first, None).unwrap());

        assert!(claims.claim(IDENTITY, &second, None).is_ok());
    }

    #[test]
    fn should_keep_a_claim_whose_capture_is_durable() {
        let claims = Arc::new(ClaimedIdentities::default());
        let first = url("/product/223/table/");
        let second = url("/product/223/table-1/");

        claims.claim(IDENTITY, &first, None).unwrap().keep();

        assert_eq!(
            claims
                .claim(IDENTITY, &second, None)
                .err()
                .map(|claimant| claimant.url),
            Some(first)
        );
    }

    #[test]
    fn should_not_release_a_claim_another_url_holds() {
        let claims = Arc::new(ClaimedIdentities::default());
        let first = url("/product/223/table/");
        let second = url("/product/223/table-1/");
        claims.claim(IDENTITY, &first, None).unwrap().keep();

        claims.release(IDENTITY, &second);

        assert!(claims.claim(IDENTITY, &second, None).is_err());
    }
}
