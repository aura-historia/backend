//! Internal English translation of the semantic intent a user typed into a saved search.
//!
//! Matching classifies non-English intent against its English translation, which the classifier
//! enforces more reliably. The user's own text stays the only version anyone sees or edits; a
//! translation is matching state, like the filter's embedding, and is used only while its source
//! fingerprint still matches the text it was made from.

use domain_primitives::query::text_query::TextQuery;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, ProductListingSearch,
};
use sha2::{Digest, Sha256};

/// SHA-256 of the canonical semantic intent: every base product query in order and the enhanced
/// search description. Other criteria are structured and never translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SearchIntentFingerprint([u8; 32]);

impl SearchIntentFingerprint {
    const DOMAIN: &'static [u8] = b"aura-historia/search-intent/v1";

    pub fn of(search: &ProductListingSearch) -> Self {
        let mut digest = Sha256::new();
        digest.update(Self::DOMAIN);
        write_part(
            &mut digest,
            &search.product_listing_query.len().to_be_bytes(),
        );
        for query in &search.product_listing_query {
            write_part(&mut digest, query.as_bytes());
        }
        match &search.enhanced_search_description {
            Some(description) => {
                digest.update([1]);
                write_part(&mut digest, description.as_bytes());
            }
            None => digest.update([0]),
        }
        Self(digest.finalize().into())
    }

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Length-prefixed, so adjacent parts can never run together into the same digest input.
fn write_part(digest: &mut Sha256, part: &[u8]) {
    digest.update(part.len().to_be_bytes());
    digest.update(part);
}

/// The English rendering of a search's semantic intent, entry for entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnglishSearchIntent {
    product_queries: Vec<TextQuery<1>>,
    enhanced_search_description: Option<EnhancedSearchDescription>,
}

impl EnglishSearchIntent {
    pub fn new(
        product_queries: Vec<TextQuery<1>>,
        enhanced_search_description: Option<EnhancedSearchDescription>,
    ) -> Self {
        Self {
            product_queries,
            enhanced_search_description,
        }
    }

    pub fn product_queries(&self) -> &[TextQuery<1>] {
        &self.product_queries
    }

    pub fn enhanced_search_description(&self) -> Option<&EnhancedSearchDescription> {
        self.enhanced_search_description.as_ref()
    }

    /// The same number and order of base queries, and an enhanced description exactly when the
    /// source has one.
    fn mirrors(&self, search: &ProductListingSearch) -> bool {
        self.product_queries.len() == search.product_listing_query.len()
            && self.enhanced_search_description.is_some()
                == search.enhanced_search_description.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SearchIntentTranslationError {
    #[error("translated search intent does not mirror the source's queries and description")]
    ShapeMismatch,
}

/// A translation of one exact source intent. `english` is absent when the source is English
/// already; the marker keeps such filters from being translated again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIntentTranslation {
    source: SearchIntentFingerprint,
    english: Option<EnglishSearchIntent>,
}

impl SearchIntentTranslation {
    pub fn already_english(search: &ProductListingSearch) -> Self {
        Self {
            source: SearchIntentFingerprint::of(search),
            english: None,
        }
    }

    pub fn english(
        search: &ProductListingSearch,
        english: EnglishSearchIntent,
    ) -> Result<Self, SearchIntentTranslationError> {
        if !english.mirrors(search) {
            return Err(SearchIntentTranslationError::ShapeMismatch);
        }
        Ok(Self {
            source: SearchIntentFingerprint::of(search),
            english: Some(english),
        })
    }

    /// Persisted state whose shape is checked against the search it is used with.
    pub fn rehydrate(
        source: SearchIntentFingerprint,
        english: Option<EnglishSearchIntent>,
    ) -> Self {
        Self { source, english }
    }

    pub fn source(&self) -> SearchIntentFingerprint {
        self.source
    }

    pub fn english_intent(&self) -> Option<&EnglishSearchIntent> {
        self.english.as_ref()
    }

    pub fn is_current_for(&self, search: &ProductListingSearch) -> bool {
        self.source == SearchIntentFingerprint::of(search)
    }

    /// The English intent to classify `search` with. A stale translation, made from different
    /// text, is never used; a current one that does not mirror the source is invalid state.
    pub fn english_for(
        &self,
        search: &ProductListingSearch,
    ) -> Result<Option<&EnglishSearchIntent>, SearchIntentTranslationError> {
        if !self.is_current_for(search) {
            return Ok(None);
        }
        match &self.english {
            Some(english) if !english.mirrors(search) => {
                Err(SearchIntentTranslationError::ShapeMismatch)
            }
            english => Ok(english.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;

    fn search(queries: &[&str], description: Option<&str>) -> ProductListingSearch {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        search.product_listing_query = queries
            .iter()
            .map(|query| TextQuery::try_from(*query).unwrap())
            .collect();
        search.enhanced_search_description =
            description.map(|text| EnhancedSearchDescription::try_from(text).unwrap());
        search
    }

    fn english(queries: &[&str], description: Option<&str>) -> EnglishSearchIntent {
        let search = search(queries, description);
        EnglishSearchIntent::new(
            search.product_listing_query,
            search.enhanced_search_description,
        )
    }

    #[test]
    fn should_fingerprint_only_semantic_intent() {
        let original = search(&["Manschettenknöpfe Gold"], Some("mit rotem Stein"));
        let mut other_criteria = original.clone();
        other_criteria.language = Language::De;
        other_criteria.currency = Currency::Usd;

        assert_eq!(
            SearchIntentFingerprint::of(&original),
            SearchIntentFingerprint::of(&other_criteria)
        );
    }

    #[test]
    fn should_distinguish_query_boundaries_order_and_description() {
        let fingerprints = [
            search(&["ab", "c"], None),
            search(&["a", "bc"], None),
            search(&["c", "ab"], None),
            search(&["ab c"], None),
            search(&["ab", "c"], Some("c")),
            search(&[], Some("ab")),
            search(&["ab"], None),
        ]
        .iter()
        .map(SearchIntentFingerprint::of)
        .collect::<std::collections::HashSet<_>>();

        assert_eq!(7, fingerprints.len());
    }

    #[test]
    fn should_use_a_current_english_translation() {
        let source = search(&["Manschettenknöpfe Gold"], Some("mit rotem Stein"));
        let translation =
            SearchIntentTranslation::english(&source, english(&["gold cufflinks"], Some("red")))
                .unwrap();

        let used = translation.english_for(&source).unwrap();

        assert_eq!(
            Some("gold cufflinks"),
            used.map(|intent| intent.product_queries()[0].as_ref())
        );
    }

    #[test]
    fn should_never_use_a_stale_translation() {
        let source = search(&["Manschettenknöpfe Gold"], None);
        let translation =
            SearchIntentTranslation::english(&source, english(&["gold cufflinks"], None)).unwrap();
        let edited = search(&["Manschettenknöpfe Silber"], None);

        assert!(!translation.is_current_for(&edited));
        assert_eq!(Ok(None), translation.english_for(&edited));
    }

    #[test]
    fn should_classify_english_text_as_typed() {
        let source = search(&["gold cufflinks"], None);
        let translation = SearchIntentTranslation::already_english(&source);

        assert!(translation.is_current_for(&source));
        assert_eq!(Ok(None), translation.english_for(&source));
    }

    #[test]
    fn should_reject_translations_that_do_not_mirror_the_source() {
        let source = search(&["Gold", "Silber"], Some("mit Stein"));
        for mismatched in [
            english(&["gold"], Some("with stone")),
            english(&["gold", "silver", "brass"], Some("with stone")),
            english(&["gold", "silver"], None),
        ] {
            assert_eq!(
                Err(SearchIntentTranslationError::ShapeMismatch),
                SearchIntentTranslation::english(&source, mismatched.clone())
            );
            let persisted = SearchIntentTranslation::rehydrate(
                SearchIntentFingerprint::of(&source),
                Some(mismatched),
            );
            assert_eq!(
                Err(SearchIntentTranslationError::ShapeMismatch),
                persisted.english_for(&source)
            );
        }
    }
}
