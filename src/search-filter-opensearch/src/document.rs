use common::currency::domain::Currency;
use common::language::domain::Language;
use common::resource_state::document::ResourceStateDocument;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use product_core::product_search::ProductSearch;
use search_filter_service::ports::SearchFilterView;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchFilterDocument {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: ResourceStateDocument,
    pub search: serde_json::Value,
    pub query: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_hybrid_search_matched: OffsetDateTime,
}

impl From<&SearchFilterView> for SearchFilterDocument {
    fn from(view: &SearchFilterView) -> Self {
        Self {
            user_search_filter_id: view.search_filter_id,
            user_id: view.user_id,
            name: view.name.clone(),
            notifications: view.notifications,
            state: view.state.into(),
            search: json!({
                "language": view.search.language.as_str(),
                "currency": view.search.currency.as_str(),
                "enhancedSearchDescription": view.search.enhanced_search_description.as_ref().map(|v| v.as_ref()),
            }),
            query: build_percolator_query(&view.search),
            embedding: view.embedding.clone(),
            created: view.created,
            updated: view.updated,
            last_hybrid_search_matched: view.last_hybrid_search_matched,
        }
    }
}

impl From<SearchFilterDocument> for SearchFilterView {
    fn from(document: SearchFilterDocument) -> Self {
        SearchFilterView {
            search_filter_id: document.user_search_filter_id,
            user_id: document.user_id,
            name: document.name,
            notifications: document.notifications,
            state: document.state.into(),
            search: ProductSearch::new(Language::default(), Currency::default()),
            embedding: document.embedding,
            created: document.created,
            updated: document.updated,
            last_hybrid_search_matched: document.last_hybrid_search_matched,
        }
    }
}

pub(crate) fn build_percolator_query(search: &ProductSearch) -> serde_json::Value {
    let must: Vec<_> = search
        .product_query
        .iter()
        .map(|text| {
            json!({
                "multi_match": {
                    "query": text.to_string(),
                    "fields": ["titleNative.text", "titleDe", "titleEn", "titleFr", "titleEs"]
                }
            })
        })
        .collect();
    json!({"bool":{"must": must}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::language::domain::Language;
    use common::resource_state::domain::ResourceState;
    use common::user_search_filter_id::UserSearchFilterId;
    use common::user_search_filter_name::UserSearchFilterName;
    #[test]
    fn should_map_scalar_view_fields_to_document_with_metadata() {
        let now = OffsetDateTime::now_utc();
        let view = SearchFilterView {
            search_filter_id: UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("daily"),
            notifications: true,
            state: ResourceState::Active,
            search: ProductSearch::new(Language::En, Currency::Eur),
            embedding: Some(vec![1.0]),
            created: now,
            updated: now,
            last_hybrid_search_matched: OffsetDateTime::UNIX_EPOCH,
        };

        let document = SearchFilterDocument::from(&view);

        assert_eq!(view.search_filter_id, document.user_search_filter_id);
        assert_eq!(view.user_id, document.user_id);
        assert_eq!(view.name, document.name);
        assert_eq!(view.notifications, document.notifications);
        assert_eq!(ResourceStateDocument::from(view.state), document.state);
        assert_eq!(now, document.created);
        assert_eq!(Some(vec![1.0]), document.embedding);
    }

    #[test]
    fn should_build_percolator_query() {
        let query = build_percolator_query(&ProductSearch::new(Language::En, Currency::Eur));
        assert_eq!(serde_json::json!({"bool":{"must":[]}}), query);
    }
}
