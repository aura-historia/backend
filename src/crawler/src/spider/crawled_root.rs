//! The crawl root a spider run crawled. Registering a domain again with another crawl root moves
//! the domain to it and clears the old root's crawl state, while a run of the old root may still
//! be crawling (`docs/crawler/operations.md#manual-authoring`).

use crate::CrawlerDomainId;
use url::Url;

/// The domain a spider run crawled and the crawl root host it crawled, as the domain was
/// registered when the run started. Every write of the run's outcome to the domain (its crawl
/// state, frontier and last full crawl) compares this root with the domain's in the same
/// statement or transaction, and records nothing once the domain has another one. The stamp is
/// the host, so a domain moved back to the crawled root before the run ends records the run's
/// outcome again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawledRoot {
    pub domain_id: CrawlerDomainId,
    pub crawl_root_host: String,
}

impl CrawledRoot {
    pub fn new(domain_id: CrawlerDomainId, crawl_root_host: impl Into<String>) -> Self {
        Self {
            domain_id,
            crawl_root_host: crawl_root_host.into(),
        }
    }

    /// The root of a run that crawls from `root_url`, or `None` when the URL names no host.
    pub fn of_root_url(domain_id: CrawlerDomainId, root_url: &Url) -> Option<Self> {
        root_url
            .host_str()
            .map(|crawl_root_host| Self::new(domain_id, crawl_root_host))
    }
}

/// Whether a write of a spider run's outcome reached the run's domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlOutcomeWrite {
    /// The domain still has the crawled root, and the outcome is recorded.
    Recorded,
    /// The domain moved to another crawl root after the run started, or was removed. Nothing was
    /// written: the run's outcome describes a root the domain no longer has.
    RootMoved,
}

impl CrawlOutcomeWrite {
    /// The write of an `UPDATE` guarded by the crawled root, from the rows it changed.
    pub(crate) fn of_guarded_update(rows_affected: u64) -> Self {
        match rows_affected {
            0 => Self::RootMoved,
            _ => Self::Recorded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_take_the_host_a_run_crawls_from_its_root_url() {
        let domain_id = CrawlerDomainId::new();

        assert_eq!(
            CrawledRoot::of_root_url(domain_id, &Url::parse("https://www.example.com").unwrap()),
            Some(CrawledRoot::new(domain_id, "www.example.com"))
        );
        assert_eq!(
            CrawledRoot::of_root_url(domain_id, &Url::parse("mailto:shop@example.com").unwrap()),
            None
        );
    }
}
