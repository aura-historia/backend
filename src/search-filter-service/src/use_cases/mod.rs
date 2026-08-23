use embedding::{EmbeddingError, EmbeddingText};
use product_core::product_search::ProductSearch;

mod create_search_filter;
mod delete_owned_search_filter;
mod generate_search_filter_match_notification;
mod get_owned_search_filter;
mod list_owned_search_filters;
mod list_search_filter_matches;
mod match_product_event;
mod project_search_filter_change;
mod run_periodic_search_filter_matching;
mod update_owned_search_filter;
mod update_search_filter_match_feedback;

pub use create_search_filter::*;
pub use delete_owned_search_filter::*;
pub use generate_search_filter_match_notification::*;
pub use get_owned_search_filter::*;
pub use list_owned_search_filters::*;
pub use list_search_filter_matches::*;
pub use match_product_event::*;
pub use project_search_filter_change::*;
pub use run_periodic_search_filter_matching::*;
pub use update_owned_search_filter::*;
pub use update_search_filter_match_feedback::*;

pub(crate) fn embedding_query(
    search: &ProductSearch,
) -> Result<Option<EmbeddingText>, EmbeddingError> {
    let mut parts = search
        .product_query
        .iter()
        .map(AsRef::as_ref)
        .filter(|value: &&str| !value.trim().is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(description) = &search.enhanced_search_description {
        parts.push(description.as_ref().to_owned());
    }
    let Some(text) = (!parts.is_empty()).then(|| parts.join("\n")) else {
        return Ok(None);
    };

    EmbeddingText::new(text).map(Some)
}

#[cfg(test)]
mod tests {
    use super::embedding_query;
    use localization::Language;
    use money::Currency;
    use product_core::product_search::ProductSearch;

    #[test]
    fn should_build_search_embedding_query() -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query("vintage brass lamp".try_into()?);

        let query = embedding_query(&search)?;

        assert!(matches!(
            query,
            Some(ref text) if text.as_str() == "vintage brass lamp"
        ));
        Ok(())
    }

    #[test]
    fn should_not_build_search_embedding_query_without_search_terms()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductSearch::new(Language::En, Currency::Eur);

        assert_eq!(None, embedding_query(&search)?);
        Ok(())
    }
}
