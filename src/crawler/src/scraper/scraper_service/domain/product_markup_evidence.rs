//! Whether a ListingSource's own product pages make the absence of Product markup evidence that a
//! page is no product page (`docs/crawler/extraction-and-review.md#product-markup-pre-check`).

/// The fewest product pages of a ListingSource whose markup verdict must be recorded before a
/// failing page may be classified by its markup: below it, a shop whose few scraped pages happen
/// to carry markup says nothing about the pages that do not.
pub const MIN_PRODUCT_PAGES_WITH_VERDICT: u64 = 20;

/// The Product markup verdicts recorded on a ListingSource's live product URLs, each as of the
/// URL's latest successful scrape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductMarkupEvidence {
    /// Live product URLs whose latest successful scrape recorded a verdict.
    pub pages_with_verdict: u64,
    /// The part of them whose page described exactly one schema.org `Product`.
    pub pages_describing_one_product: u64,
}

impl ProductMarkupEvidence {
    /// Whether the ListingSource reliably marks its product pages up: at least
    /// [`MIN_PRODUCT_PAGES_WITH_VERDICT`] pages carry a verdict and every one of them describes
    /// exactly one Product. A single scraped product page without that markup shows that the
    /// shop's product pages can lack it, so its absence on another page proves nothing.
    pub fn marks_products_reliably(self) -> bool {
        self.pages_with_verdict >= MIN_PRODUCT_PAGES_WITH_VERDICT
            && self.pages_describing_one_product == self.pages_with_verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(
        pages_with_verdict: u64,
        pages_describing_one_product: u64,
    ) -> ProductMarkupEvidence {
        ProductMarkupEvidence {
            pages_with_verdict,
            pages_describing_one_product,
        }
    }

    #[test]
    fn should_rely_on_markup_when_every_page_at_the_threshold_describes_one_product() {
        assert!(
            evidence(
                MIN_PRODUCT_PAGES_WITH_VERDICT,
                MIN_PRODUCT_PAGES_WITH_VERDICT
            )
            .marks_products_reliably()
        );
    }

    #[test]
    fn should_not_rely_on_markup_when_too_few_pages_carry_a_verdict() {
        let below = MIN_PRODUCT_PAGES_WITH_VERDICT - 1;
        assert!(!evidence(below, below).marks_products_reliably());
        assert!(!evidence(0, 0).marks_products_reliably());
    }

    #[test]
    fn should_not_rely_on_markup_when_one_product_page_lacks_it() {
        assert!(!evidence(500, 499).marks_products_reliably());
    }
}
