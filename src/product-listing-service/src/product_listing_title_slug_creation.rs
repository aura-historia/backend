use product_listing_core::product_listing_slug_id::{
    InvalidProductListingSlugId, ProductListingSlugId,
};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS: usize = 5;

/// Supplies one public title-slug candidate for each ProductListing creation attempt.
pub trait ProductListingTitleSlugGenerator: Send + Sync {
    fn generate(&self, title: &str) -> Result<ProductListingSlugId, InvalidProductListingSlugId>;
}

/// Production generator with a random, lowercase hexadecimal suffix.
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomProductListingTitleSlugGenerator;

impl ProductListingTitleSlugGenerator for RandomProductListingTitleSlugGenerator {
    fn generate(&self, title: &str) -> Result<ProductListingSlugId, InvalidProductListingSlugId> {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        ProductListingSlugId::from_title_and_suffix(title, &suffix[..6])
    }
}

/// Deterministic title-slug generator for focused service tests.
#[derive(Debug)]
pub struct SequenceProductListingTitleSlugGenerator {
    candidates: Mutex<VecDeque<ProductListingSlugId>>,
}

impl SequenceProductListingTitleSlugGenerator {
    pub fn new(candidates: impl IntoIterator<Item = ProductListingSlugId>) -> Self {
        Self {
            candidates: Mutex::new(candidates.into_iter().collect()),
        }
    }
}

impl ProductListingTitleSlugGenerator for SequenceProductListingTitleSlugGenerator {
    fn generate(&self, _title: &str) -> Result<ProductListingSlugId, InvalidProductListingSlugId> {
        let mut candidates = match self.candidates.lock() {
            Ok(candidates) => candidates,
            Err(poisoned) => poisoned.into_inner(),
        };
        candidates.pop_front().ok_or(InvalidProductListingSlugId)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleSlugCollisionRetry {
    Retry,
    Exhausted,
    DoNotRetry,
}

pub(crate) fn title_slug_collision_retry(
    attempt: usize,
    collision: bool,
) -> TitleSlugCollisionRetry {
    if !collision {
        return TitleSlugCollisionRetry::DoNotRetry;
    }
    if attempt < MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS {
        TitleSlugCollisionRetry::Retry
    } else {
        TitleSlugCollisionRetry::Exhausted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(suffix: &str) -> ProductListingSlugId {
        ProductListingSlugId::from_title_and_suffix("listing", suffix)
            .unwrap_or_else(|error| panic!("valid test slug: {error}"))
    }

    #[test]
    fn should_supply_candidates_in_sequence_order() {
        let generator =
            SequenceProductListingTitleSlugGenerator::new([slug("000001"), slug("000002")]);

        assert_eq!(generator.generate("ignored"), Ok(slug("000001")));
        assert_eq!(generator.generate("ignored"), Ok(slug("000002")));
    }

    #[test]
    fn should_retry_title_slug_collision_before_attempt_limit() {
        assert_eq!(
            title_slug_collision_retry(1, true),
            TitleSlugCollisionRetry::Retry
        );
    }

    #[test]
    fn should_exhaust_title_slug_collision_at_attempt_limit() {
        assert_eq!(
            title_slug_collision_retry(MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS, true),
            TitleSlugCollisionRetry::Exhausted
        );
    }

    #[test]
    fn should_not_retry_non_collision_failure() {
        assert_eq!(
            title_slug_collision_retry(1, false),
            TitleSlugCollisionRetry::DoNotRetry
        );
    }
}
