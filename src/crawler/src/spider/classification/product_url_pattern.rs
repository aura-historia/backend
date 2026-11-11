//! The regexes a ListingSource's product URLs are recognised by.

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use strum::IntoEnumIterator;
use thiserror::Error;

/// What a product URL pattern is matched against, as `listing_sources.url_pattern_matching`
/// stores it with the pattern. The contract is in `docs/crawler/extraction-and-review.md`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::EnumIter,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UrlPatternMatching {
    /// The pattern and its exclusion match anywhere in the whole URL.
    #[default]
    FullUrlSearch,
    /// The pattern matches the whole of the URL's path and `?query`, without scheme, host, port
    /// or fragment; its exclusion matches anywhere in that path and query.
    PathAndQueryFullMatch,
}

impl UrlPatternMatching {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullUrlSearch => "FULL_URL_SEARCH",
            Self::PathAndQueryFullMatch => "PATH_AND_QUERY_FULL_MATCH",
        }
    }

    /// Decodes the persisted value, refusing one this crawler does not know.
    pub fn from_persisted(value: &str) -> Result<Self, ProductUrlPatternError> {
        Self::iter()
            .find(|matching| matching.as_str() == value)
            .ok_or_else(|| ProductUrlPatternError::UnknownMatching(value.to_owned()))
    }

    /// Decodes the persisted value of a column that is `NULL` without a pattern.
    pub fn from_persisted_column(
        value: Option<&str>,
    ) -> Result<Option<Self>, ProductUrlPatternError> {
        value.map(Self::from_persisted).transpose()
    }

    pub fn is_full_url_search(&self) -> bool {
        *self == Self::FullUrlSearch
    }

    /// The text of `spelling`, a URL, that a pattern of this scope is matched against: the
    /// whole URL, or its path and `?query` (the path `/` for an empty one). `None` for a
    /// path-and-query scope and a spelling that is no URL, which no pattern takes.
    fn target(self, spelling: &str) -> Option<Cow<'_, str>> {
        match self {
            Self::FullUrlSearch => Some(Cow::Borrowed(spelling)),
            Self::PathAndQueryFullMatch => url::Url::parse(spelling)
                .ok()
                .map(|url| Cow::Owned(path_and_query(&url))),
        }
    }
}

/// `url`'s path and `?query`, without scheme, host, port or fragment: what a
/// [`UrlPatternMatching::PathAndQueryFullMatch`] pattern is matched against. An empty path reads
/// as `/` and an empty query is left out.
pub fn path_and_query(url: &url::Url) -> String {
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    match url.query().filter(|query| !query.is_empty()) {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    }
}

/// A ListingSource's product URL pattern. A URL on the crawl host is a product URL when its
/// page matches `include` and does not match `exclude`, the regex for the few non-items that
/// share the product URL shape (gift cards, vouchers, services). Under
/// [`UrlPatternMatching::FullUrlSearch`] both match anywhere in the URL; under
/// [`UrlPatternMatching::PathAndQueryFullMatch`] `include` matches the whole path and query and
/// `exclude` anywhere in it. The contract is in `docs/crawler/extraction-and-review.md`.
#[derive(Debug, Clone)]
pub struct ProductUrlPattern {
    include: Regex,
    matching: UrlPatternMatching,
    /// `include` anchored to the whole text it is matched against, under a full-match scope.
    anchored: Option<Regex>,
    exclude: Option<Regex>,
}

#[derive(Debug, Error)]
pub enum ProductUrlPatternError {
    #[error("invalid URL pattern: {0}")]
    InvalidRegex(#[from] regex::Error),
    /// A blank regex matches every URL, so a blank exclusion would take every product away.
    #[error("the URL pattern's exclusion is blank")]
    BlankExclude,
    /// A path-and-query regex that names a scheme or a host matches no URL's path and query.
    #[error("a path-and-query URL pattern must not name a scheme or host: {0}")]
    NamesSchemeOrHost(String),
    #[error("unknown URL pattern matching: {0}")]
    UnknownMatching(String),
    /// A stored pattern always has its matching.
    #[error("the stored URL pattern has no matching")]
    MissingMatching,
}

impl ProductUrlPattern {
    /// A pattern matched anywhere in the whole URL.
    pub fn new(include: Regex) -> Self {
        Self {
            include,
            matching: UrlPatternMatching::FullUrlSearch,
            anchored: None,
            exclude: None,
        }
    }

    /// Decodes a pattern without an exclusion, matched anywhere in the whole URL, from the regex
    /// source the database or a review decision holds.
    pub fn parse(include: &str) -> Result<Self, ProductUrlPatternError> {
        Self::parse_matching(include, UrlPatternMatching::FullUrlSearch)
    }

    /// Decodes a pattern without an exclusion, matched as `matching` says. A path-and-query
    /// pattern is matched anchored, so one that anchors itself with `^…$` matches the same.
    pub fn parse_matching(
        include: &str,
        matching: UrlPatternMatching,
    ) -> Result<Self, ProductUrlPatternError> {
        let pattern = Self::new(Regex::new(include)?);
        match matching {
            UrlPatternMatching::FullUrlSearch => Ok(pattern),
            UrlPatternMatching::PathAndQueryFullMatch => {
                refuse_scheme_or_host(include)?;
                Ok(Self {
                    matching,
                    anchored: Some(Regex::new(&format!("^(?:{include})$"))?),
                    ..pattern
                })
            }
        }
    }

    /// The pattern a ListingSource row stores: `None` without a pattern. An exclusion or a
    /// matching without a pattern is no row the schema allows and takes nothing; a pattern
    /// without its matching is refused.
    pub fn from_stored(
        include: Option<&str>,
        exclude: Option<&str>,
        matching: Option<UrlPatternMatching>,
    ) -> Result<Option<Self>, ProductUrlPatternError> {
        include
            .map(|include| {
                let matching = matching.ok_or(ProductUrlPatternError::MissingMatching)?;
                Self::parse_matching(include, matching)?.with_exclude(exclude)
            })
            .transpose()
    }

    /// This pattern with the exclusion `exclude`, or without one for `None`.
    pub fn with_exclude(self, exclude: Option<&str>) -> Result<Self, ProductUrlPatternError> {
        let exclude = exclude
            .map(|source| {
                if source.trim().is_empty() {
                    return Err(ProductUrlPatternError::BlankExclude);
                }
                if self.matching == UrlPatternMatching::PathAndQueryFullMatch {
                    refuse_scheme_or_host(source)?;
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

    /// What the pattern is matched against, as `listing_sources.url_pattern_matching` stores it.
    pub fn matching(&self) -> UrlPatternMatching {
        self.matching
    }

    /// Whether `other` is the same pattern: the same regex sources for the pattern and the
    /// exclusion, matched the same way.
    pub fn same_regexes(&self, other: &Self) -> bool {
        self.include_source() == other.include_source()
            && self.exclude_source() == other.exclude_source()
            && self.matching == other.matching
    }

    /// Whether the pattern takes the one page that goes by the URL `spellings`: one of them
    /// matches the pattern and none matches the exclusion, so an exclusion written for one
    /// spelling takes the page in every spelling.
    pub fn takes_page<'a>(&self, spellings: impl IntoIterator<Item = &'a str> + Clone) -> bool {
        let include = self.anchored.as_ref().unwrap_or(&self.include);
        spellings.clone().into_iter().any(|spelling| {
            self.matching
                .target(spelling)
                .is_some_and(|target| include.is_match(&target))
        }) && !self.excludes_page(spellings)
    }

    /// Whether the exclusion takes the page that goes by the URL `spellings`.
    pub fn excludes_page<'a>(&self, spellings: impl IntoIterator<Item = &'a str>) -> bool {
        self.exclude.as_ref().is_some_and(|exclude| {
            spellings.into_iter().any(|spelling| {
                self.matching
                    .target(spelling)
                    .is_some_and(|target| exclude.is_match(&target))
            })
        })
    }
}

/// Refuses a path-and-query regex that names a scheme (`://`) or a host (a leading `//`).
fn refuse_scheme_or_host(source: &str) -> Result<(), ProductUrlPatternError> {
    if source.contains("://") || source.trim_start_matches('^').starts_with("//") {
        return Err(ProductUrlPatternError::NamesSchemeOrHost(source.to_owned()));
    }
    Ok(())
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

    fn path_and_query(include: &str, exclude: Option<&str>) -> ProductUrlPattern {
        ProductUrlPattern::parse_matching(include, UrlPatternMatching::PathAndQueryFullMatch)
            .and_then(|pattern| pattern.with_exclude(exclude))
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

    /// The paper's f(u): the pattern matches the whole path and query, never scheme, host, port
    /// or fragment, so a pattern anchored by `^…$` or not matches the same, and the exclusion is
    /// a search within the path and query.
    #[test]
    fn should_full_match_the_path_and_query_and_search_the_exclusion_within_it() {
        let pattern = path_and_query(r"/products/[^/?]+(\?variant=\d+)?", Some("gift-card"));
        let anchored = path_and_query(r"^/products/[^/?]+(\?variant=\d+)?$", None);

        for taking in [&pattern, &anchored] {
            assert!(taking.takes_page(["https://shop.example/products/ring"]));
            assert!(taking.takes_page(["https://shop.example:8443/products/ring?variant=7#top"]));
            assert!(!taking.takes_page(["https://shop.example/en/products/ring"]));
            assert!(!taking.takes_page(["https://shop.example/products/ring/reviews"]));
            assert!(!taking.takes_page(["https://shop.example/products/ring?page=2"]));
        }
        assert!(!pattern.takes_page(["https://shop.example/products/gift-card"]));
        assert!(path_and_query("/", None).takes_page(["https://shop.example"]));
        assert!(!path_and_query("shop", None).takes_page(["https://shop.example/"]));
    }

    #[test]
    fn should_match_the_full_url_search_anywhere_in_the_url() {
        let pattern = ProductUrlPattern::parse(r"shop\.example/products/").unwrap();

        assert!(pattern.takes_page(["https://shop.example/products/ring"]));
        assert!(pattern.takes_page(["https://shop.example/en/x?next=shop.example/products/"]));
        assert_eq!(pattern.matching(), UrlPatternMatching::FullUrlSearch);
    }

    #[test]
    fn should_refuse_a_path_and_query_pattern_that_names_a_scheme_or_host() {
        let parse = |include| {
            ProductUrlPattern::parse_matching(include, UrlPatternMatching::PathAndQueryFullMatch)
        };

        for named in [
            r"https://shop\.example/p/.+",
            r"//shop\.example/p/.+",
            r"^//shop/p",
        ] {
            assert!(
                matches!(
                    parse(named),
                    Err(ProductUrlPatternError::NamesSchemeOrHost(_))
                ),
                "{named}"
            );
        }
        assert!(
            parse("/p/.+")
                .and_then(|pattern| pattern.with_exclude(Some("https://")))
                .is_err()
        );
        assert!(ProductUrlPattern::parse(r"https://shop\.example/p/").is_ok());
    }

    #[test]
    fn should_persist_the_matching_in_screaming_snake_case_and_refuse_a_stored_pattern_without_it()
    {
        for matching in UrlPatternMatching::iter() {
            assert_eq!(
                serde_json::to_value(matching).ok(),
                Some(serde_json::json!(matching.as_str()))
            );
            assert_eq!(
                UrlPatternMatching::from_persisted(matching.as_str()).ok(),
                Some(matching)
            );
        }
        assert!(UrlPatternMatching::from_persisted("full_url_search").is_err());
        assert!(matches!(
            ProductUrlPattern::from_stored(Some("/p/"), None, None),
            Err(ProductUrlPatternError::MissingMatching)
        ));
        assert!(
            ProductUrlPattern::from_stored(None, None, None).is_ok_and(|pattern| pattern.is_none())
        );
    }
}
