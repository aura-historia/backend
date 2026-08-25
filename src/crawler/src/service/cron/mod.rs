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
    use crate::service::product_push::MockProductListingPushService;
    use crate::service::shop_registration::{
        MockShopRegistrationRepository, MockShopRegistrationSource, ShopRegistrationService,
    };
    use shop_core::shop_id::ShopId;
    use shop_core::shop_type::ShopType;

    pub(super) fn noop_shop_registration() -> ShopRegistrationService {
        let mut source = MockShopRegistrationSource::new();
        source
            .expect_fetch_registered_shops()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        let repository = MockShopRegistrationRepository::new();
        ShopRegistrationService::new(Box::new(source), Box::new(repository))
    }

    pub(super) fn noop_product_push() -> Box<MockProductListingPushService> {
        let mut push = MockProductListingPushService::new();
        push.expect_push()
            .returning(|products| Box::pin(async move { vec![true; products.len()] }));
        Box::new(push)
    }

    pub(super) fn scraper_candidate(
        shop_name: &str,
        shop_type: ShopType,
        url: url::Url,
    ) -> ScraperCandidate {
        ScraperCandidate {
            shop_id: ShopId::new(),
            shop_name: shop_name.to_string(),
            shop_type,
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
            last_scraped_state: None,
        }
    }
}
