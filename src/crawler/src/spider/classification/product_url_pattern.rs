//! The regexes a ListingSource's product URLs are recognised by.

use regex::Regex;
use thiserror::Error;

/// A ListingSource's product URL pattern. A URL on the crawl host is a product URL when its
/// page matches `include` and does not match `exclude`, the regex for the few non-items that
/// share the product URL shape (gift cards, vouchers, services). Both match anywhere in the
/// URL. The contract is in `docs/crawler/extraction-and-review.md`.
#[derive(Debug, Clone)]
pub struct ProductUrlPattern {
    include: Regex,
    exclude: Option<Regex>,
}

#[derive(Debug, Error)]
pub enum ProductUrlPatternError {
    #[error("invalid URL pattern: {0}")]
    InvalidRegex(#[from] regex::Error),
    /// A blank regex matches every URL, so a blank exclusion would take every product away.
    #[error("the URL pattern's exclusion is blank")]
    BlankExclude,
}

impl ProductUrlPattern {
    pub fn new(include: Regex) -> Self {
        Self {
            include,
            exclude: None,
        }
    }

    /// Decodes a pattern without an exclusion from the regex source the database or a review
    /// decision holds.
    pub fn parse(include: &str) -> Result<Self, ProductUrlPatternError> {
        Ok(Self::new(Regex::new(include)?))
    }

    /// The pattern a ListingSource row stores: `None` without a pattern. An exclusion without a
    /// pattern is no row the schema allows and takes nothing.
    pub fn from_stored(
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Option<Self>, ProductUrlPatternError> {
        include
            .map(|include| Self::parse(include)?.with_exclude(exclude))
            .transpose()
    }

    /// This pattern with the exclusion `exclude`, or without one for `None`.
    pub fn with_exclude(self, exclude: Option<&str>) -> Result<Self, ProductUrlPatternError> {
        let exclude = exclude
            .map(|source| {
                if source.trim().is_empty() {
                    return Err(ProductUrlPatternError::BlankExclude);
                }
                Ok(Regex::new(source)?)
            })
            .transpose()?;
        Ok(Self { exclude, ..self })
    }

    /// The regex source of the pattern, as `listing_sources.url_pattern` stores it.
    pub fn include_source(&self) -> &str {
        self.include.as_str()
    }

    /// The regex source of the exclusion, as `listing_sources.url_pattern_exclude` stores it.
    pub fn exclude_source(&self) -> Option<&str> {
        self.exclude.as_ref().map(Regex::as_str)
    }

    /// Whether `other` is the same pattern, the same regex sources for the pattern and the
    /// exclusion.
    pub fn same_regexes(&self, other: &Self) -> bool {
        self.include_source() == other.include_source()
            && self.exclude_source() == other.exclude_source()
    }

    /// Whether the pattern takes the one page that goes by the URL `spellings`: one of them
    /// matches the pattern and none matches the exclusion, so an exclusion written for one
    /// spelling takes the page in every spelling.
    pub fn takes_page<'a>(&self, spellings: impl IntoIterator<Item = &'a str> + Clone) -> bool {
        spellings
            .clone()
            .into_iter()
            .any(|spelling| self.include.is_match(spelling))
            && !self.excludes_page(spellings)
    }

    /// Whether the exclusion takes the page that goes by the URL `spellings`.
    pub fn excludes_page<'a>(&self, spellings: impl IntoIterator<Item = &'a str>) -> bool {
        self.exclude.as_ref().is_some_and(|exclude| {
            spellings
                .into_iter()
                .any(|spelling| exclude.is_match(spelling))
        })
    }
}

impl From<Regex> for ProductUrlPattern {
    fn from(include: Regex) -> Self {
        Self::new(include)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(include: &str, exclude: &str) -> ProductUrlPattern {
        ProductUrlPattern::parse(include)
            .and_then(|pattern| pattern.with_exclude(Some(exclude)))
            .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn should_take_a_page_that_matches_the_pattern_and_not_the_exclusion() {
        let pattern = pattern("/products/", "gift-card|-service$");

        assert!(pattern.takes_page(["https://shop.example/products/antique-ring"]));
        assert!(
            !pattern.takes_page(["https://shop.example/products/the-antique-ring-shop-gift-card"])
        );
        assert!(!pattern.takes_page(["https://shop.example/products/ring-re-size-service"]));
        assert!(!pattern.takes_page(["https://shop.example/pages/about"]));
    }

    #[test]
    fn should_exclude_a_page_whichever_spelling_the_exclusion_matches() {
        let pattern = pattern("/products/", "voucher/$");

        assert!(!pattern.takes_page([
            "https://shop.example/products/voucher",
            "https://shop.example/products/voucher/",
        ]));
    }

    #[test]
    fn should_refuse_a_blank_or_invalid_exclusion() {
        let take = |exclude| ProductUrlPattern::parse("/p/").and_then(|p| p.with_exclude(exclude));

        assert!(matches!(
            take(Some(" ")),
            Err(ProductUrlPatternError::BlankExclude)
        ));
        assert!(matches!(
            take(Some("(")),
            Err(ProductUrlPatternError::InvalidRegex(_))
        ));
        assert!(take(None).is_ok_and(|pattern| pattern.exclude_source().is_none()));
    }
}
