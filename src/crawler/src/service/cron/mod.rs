mod config;
mod job;
mod metrics;
mod scraper;
mod spider;

pub use config::CrawlerCronConfig;
pub use job::CrawlerCronJob;

#[cfg(test)]
pub(super) mod test_support {
    use crate::CrawlerDomainId;
    use crate::scraper::candidate_service::{DomainHealthSnapshot, ScraperCandidate};
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, MockListingSourceRegistrationRepository,
        MockListingSourceRegistrationSource,
    };
    use crate::service::raw_capture::{
        MockProductListingRawCaptureService, ProductListingRawCaptureOutcome,
    };
    use listing_source_core::ListingSourceId;
    use std::hash::{Hash, Hasher};

    pub(super) fn noop_listing_source_registration() -> ListingSourceRegistrationService {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_apply_snapshot()
            .returning(|_| {
                Box::pin(async {
                    Ok(crate::service::listing_source_registration::ListingSourceSnapshotResult::default())
                })
            });
        ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
    }

    pub(super) fn noop_raw_capture() -> Box<MockProductListingRawCaptureService> {
        let mut capture = MockProductListingRawCaptureService::new();
        capture.expect_capture().returning(|observations| {
            Box::pin(
                async move { vec![ProductListingRawCaptureOutcome::Persisted; observations.len()] },
            )
        });
        Box::new(capture)
    }

    pub(super) fn scraper_candidate(listing_source_name: &str, url: url::Url) -> ScraperCandidate {
        let domain = url
            .host_str()
            .unwrap_or_default()
            .trim_start_matches("www.")
            .to_owned();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        domain.hash(&mut hasher);
        let bits = hasher.finish();
        let mut raw = u128::from(bits) << 64 | u128::from(bits);
        raw = (raw & !(0xf_u128 << 76)) | (7_u128 << 76);
        raw = (raw & !(0x3_u128 << 62)) | (2_u128 << 62);
        let raw_uuid = uuid::Uuid::from_u128(raw);
        let domain_id = CrawlerDomainId::try_from(raw_uuid)
            .expect("test domain UUID should use the UUIDv7 representation");
        ScraperCandidate {
            listing_source_id: ListingSourceId::new(),
            domain_id,
            listing_source_domain: domain,
            listing_source_name: listing_source_name.to_string(),
            fallback_currency: None,
            url_pattern: None,
            url: url.clone(),
            source_record_key: url.to_string(),
            last_source_listing_id: None,
            last_scraped_hash: None,
            last_scraped_schema_fingerprint: None,
            last_captured_raw_input_sha256: None,
            domain_health: DomainHealthSnapshot {
                scrape_failure_streak: 0,
                last_scrape_error_kind: None,
                next_scrape_at: None,
            },
            is_domain_probe: false,
        }
    }
}
