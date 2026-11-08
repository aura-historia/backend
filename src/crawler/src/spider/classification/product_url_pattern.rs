//! The regex a ListingSource's product URLs are recognised by.

use regex::Regex;

/// A ListingSource's product URL pattern: the regex a product URL matches anywhere in the URL.
#[derive(Debug, Clone)]
pub struct ProductUrlPattern {
    include: Regex,
}

impl ProductUrlPattern {
    pub fn new(include: Regex) -> Self {
        Self { include }
    }

    /// Decodes a pattern from the regex source the database or a review decision holds.
    pub fn parse(include: &str) -> Result<Self, regex::Error> {
        Regex::new(include).map(Self::new)
    }

    /// The regex source of the pattern, as `listing_sources.url_pattern` stores it.
    pub fn include_source(&self) -> &str {
        self.include.as_str()
    }

    /// Whether this pattern recognises the one `spelling` of a URL.
    pub fn takes_spelling(&self, spelling: &str) -> bool {
        self.include.is_match(spelling)
    }
}

impl From<Regex> for ProductUrlPattern {
    fn from(include: Regex) -> Self {
        Self::new(include)
    }
}
