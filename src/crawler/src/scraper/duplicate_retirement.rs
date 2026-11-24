//! Why the scraper retires a product URL as `DUPLICATE`. A retired URL is never scraped again,
//! so every retirement records the product URL it duplicates, the evidence and the time
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`), and any retirement can be found
//! and undone with one UPDATE.

use crate::scraper::same_product::SameProductEvidence;
use url::Url;

/// The evidence that a retired URL duplicates another product URL of its ListingSource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateEvidence {
    /// The URL redirects to a product URL its ListingSource already stores.
    RedirectDestination,
    /// The URL's page shows the identity the other URL carries, and the same-product rule shows
    /// one product.
    SameProduct(SameProductEvidence),
}

impl DuplicateEvidence {
    /// The persisted value (`listing_source_urls.duplicate_evidence`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RedirectDestination => "REDIRECT_DESTINATION",
            Self::SameProduct(SameProductEvidence::Spelling) => "SPELLING",
            Self::SameProduct(SameProductEvidence::IdentityToken) => "IDENTITY_TOKEN",
            Self::SameProduct(SameProductEvidence::LanguageDirectory) => "LANGUAGE_DIRECTORY",
            Self::SameProduct(SameProductEvidence::Title) => "TITLE",
        }
    }
}

/// A retirement of a product URL as the duplicate of `duplicate_of`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateRetirement {
    pub duplicate_of: Url,
    pub evidence: DuplicateEvidence,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(DuplicateEvidence::RedirectDestination, "REDIRECT_DESTINATION")]
    #[case(
        DuplicateEvidence::SameProduct(SameProductEvidence::Spelling),
        "SPELLING"
    )]
    #[case(
        DuplicateEvidence::SameProduct(SameProductEvidence::IdentityToken),
        "IDENTITY_TOKEN"
    )]
    #[case(
        DuplicateEvidence::SameProduct(SameProductEvidence::LanguageDirectory),
        "LANGUAGE_DIRECTORY"
    )]
    #[case(DuplicateEvidence::SameProduct(SameProductEvidence::Title), "TITLE")]
    fn should_persist_each_evidence_in_screaming_snake_case(
        #[case] evidence: DuplicateEvidence,
        #[case] persisted: &str,
    ) {
        assert_eq!(evidence.as_str(), persisted);
    }
}
