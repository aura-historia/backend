mod config;
mod job;
mod metrics;
mod scraper;
mod spider;

pub use config::CrawlerCronConfig;
pub use job::CrawlerCronJob;

#[cfg(test)]
pub(super) mod test_support {
    use crate::scraper::candidate_service::ScraperCandidate;
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, MockListingSourceRegistrationRepository,
        MockListingSourceRegistrationSource,
    };
    use crate::service::product_push::MockProductListingPushService;
    use listing_source_core::ListingSourceId;

    pub(super) fn noop_listing_source_registration() -> ListingSourceRegistrationService {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_disable_listing_sources_not_in()
            .returning(|_| Box::pin(async { Ok(0) }));
        ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
    }

    pub(super) fn noop_product_push() -> Box<MockProductListingPushService> {
        let mut push = MockProductListingPushService::new();
        push.expect_push()
            .returning(|products| Box::pin(async move { vec![true; products.len()] }));
        Box::new(push)
    }

    pub(super) fn scraper_candidate(listing_source_name: &str, url: url::Url) -> ScraperCandidate {
        ScraperCandidate {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: listing_source_name.to_string(),
            url_pattern: None,
            url,
            last_scraped_hash: None,
            last_scraped_price: None,
            last_scraped_price_estimate_min: None,
            last_scraped_price_estimate_max: None,
            last_scraped_url: None,
            last_scraped_images_hash: None,
            last_scraped_auction_start: None,
            last_scraped_auction_end: None,
            last_scraped_presence: "PRESENT".to_owned(),
            last_scraped_availability: None,
        }
    }
}
