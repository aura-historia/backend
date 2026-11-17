use super::SelectorFieldEvaluation;
use crate::CrawlerReviewPageId;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::plausibility::{
    CrossPageFindings, IdentityAgreementFindings, PlausibilityFindings,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaPageReference {
    Persisted { review_page_id: CrawlerReviewPageId },
    Input { page_index: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaPageEvaluation {
    pub page_reference: SchemaPageReference,
    pub url: String,
    pub role: String,
    pub apply_ok: bool,
    pub extracted: Option<RawExtractedProduct>,
    pub error: Option<String>,
    /// Every per-page plausibility check the extraction failed. A cached matrix
    /// without them predates the checks and is refreshed from the live pages.
    pub plausibility_findings: PlausibilityFindings,
    /// The cross-page checks the extraction failed against the candidate's extractions of
    /// the other review pages. They derive from the extractions alone, so reading a cached
    /// matrix evaluates them again (`evaluate_cross_page_findings`), and a check added
    /// after the matrix was cached shows on it. A cached matrix without them predates the
    /// cross-page checks and is refreshed from the live pages.
    pub cross_page_findings: CrossPageFindings,
    /// The identities of this page that contradict the one this extraction derives: other
    /// schemas' and the captured one. A cached matrix without them predates the check and
    /// is refreshed from the live pages.
    pub identity_agreement_findings: IdentityAgreementFindings,
    pub fields: Vec<SelectorFieldEvaluation>,
}

/// Whether price text in a candidate's `state` keeps it out of approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceTextOnState {
    Blocks,
    /// The candidate's `state` rule reads a status slot: it resolves to sold or unavailable on
    /// another review page, so the amount it shows while the item is for sale is no defect.
    Accepted,
}

/// Whether an identity that is a URL or path keeps a candidate out of approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkIdentity {
    Blocks,
    /// The ListingSource's listings were captured under URL identities that the candidate keeps
    /// deriving: replacing them would leave every captured URL in an identity conflict.
    Captured,
}

/// The findings an approval accepts that would otherwise keep a candidate out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalExemptions {
    pub price_text: PriceTextOnState,
    pub link_identity: LinkIdentity,
}

impl ApprovalExemptions {
    /// Every finding that blocks approval blocks it, as deterministic approval decides.
    pub const NONE: Self = Self {
        price_text: PriceTextOnState::Blocks,
        link_identity: LinkIdentity::Blocks,
    };
}

impl SchemaPageEvaluation {
    /// Whether a failed per-page, cross-page or identity agreement check keeps the schema out
    /// of approval. A repeated image alone does not
    /// ([`blocks_approval`](crate::scraper::plausibility::PlausibilityFinding::blocks_approval)),
    /// nor does a finding `exemptions` accepts.
    pub fn blocks_approval(&self, exemptions: ApprovalExemptions) -> bool {
        !self.blocking_plausibility_findings(exemptions).is_empty()
            || !self.cross_page_findings.is_empty()
            || !self.identity_agreement_findings.is_empty()
    }

    /// The per-page findings that block approval under `exemptions`: every finding
    /// [`blocks_approval`](crate::scraper::plausibility::PlausibilityFinding::blocks_approval)
    /// keeps, except price text in `state` and an identity that links to a page of the shop
    /// where `exemptions` accepts them.
    pub fn blocking_plausibility_findings(
        &self,
        exemptions: ApprovalExemptions,
    ) -> PlausibilityFindings {
        let accepts_link_identity = self.accepts_link_identity(exemptions.link_identity);
        self.plausibility_findings.retaining(|finding| {
            finding.blocks_approval()
                && !(exemptions.price_text == PriceTextOnState::Accepted
                    && finding.is_price_text_on_state())
                && !(accepts_link_identity && finding.is_link_or_path_identity())
        })
    }

    /// Whether an approval under `link_identity` accepts this page's identity when it is a URL or
    /// path: only where the listings were captured under such identities, and only a link to a page
    /// of the shop.
    pub fn accepts_link_identity(&self, link_identity: LinkIdentity) -> bool {
        link_identity == LinkIdentity::Captured && self.identity_links_to_shop_page()
    }

    /// Whether the extracted identity links to a page of the shop, as a canonical link or
    /// `og:url` does: resolved against the page's URL it is an `http` or `https` URL on the
    /// page's host, `www.` aside, and names no image. An image `src`, a `javascript:` link or
    /// another host's URL is no URL identity a source was captured under.
    fn identity_links_to_shop_page(&self) -> bool {
        let Some(identity) = self
            .extracted
            .as_ref()
            .map(|raw| raw.source_listing_id.trim())
            .filter(|identity| !identity.is_empty())
        else {
            return false;
        };
        let Ok(page) = Url::parse(&self.url) else {
            return false;
        };
        let Ok(target) = page.join(identity) else {
            return false;
        };
        let host_of = |url: &Url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").to_ascii_lowercase())
        };
        matches!(target.scheme(), "http" | "https")
            && host_of(&target).is_some()
            && host_of(&target) == host_of(&page)
            && !names_image(target.path())
    }
}

/// Whether a URL path ends in an image file name.
fn names_image(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "avif" | "bmp" | "ico"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix cached before the cross-page or identity agreement checks lacks their
    /// findings; it must fail to load so the console refreshes it from the live pages
    /// instead of showing it as passing them.
    #[test]
    fn should_not_load_a_cached_page_evaluation_without_later_checks() {
        let page = serde_json::json!({
            "page_reference": {"kind": "input", "page_index": 0},
            "url": "https://shop.example/chair",
            "role": "SEED",
            "apply_ok": true,
            "extracted": null,
            "error": null,
            "plausibility_findings": [],
            "cross_page_findings": [],
            "identity_agreement_findings": [],
            "fields": []
        });
        assert!(serde_json::from_value::<SchemaPageEvaluation>(page.clone()).is_ok());

        for later_check in ["cross_page_findings", "identity_agreement_findings"] {
            let mut cached = page.clone();
            if let Some(fields) = cached.as_object_mut() {
                fields.remove(later_check);
            }
            assert!(
                serde_json::from_value::<SchemaPageEvaluation>(cached).is_err(),
                "a cached page without {later_check} must not load"
            );
        }
    }
}
