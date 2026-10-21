pub(super) use aura_historia_contracts::saved_searches::{
    CreateSearchFilterData, PaginatedData, SearchFilterData, SearchFilterMatchData,
    SearchFilterMatchesData, SearchFilterMatchesQuery, UpdateSearchFilterData,
    UpdateSearchFilterMatchFeedbackData,
};
use aura_historia_contracts::saved_searches::{SavedSearchInputError, SavedSearchInputErrorKind};

impl From<SavedSearchInputError> for crate::error::ApiError {
    fn from(error: SavedSearchInputError) -> Self {
        let mut mapped = Self::bad_request(match error.kind {
            SavedSearchInputErrorKind::BodyValue => crate::error::BAD_BODY_VALUE,
            SavedSearchInputErrorKind::QueryValue => crate::error::BAD_QUERY_PARAMETER_VALUE,
            SavedSearchInputErrorKind::Identifier => crate::error::INVALID_OBJECT_ID,
        })
        .with_detail(error.detail);
        if let Some(field) = error.field {
            mapped = if error.query {
                mapped.with_query_field(field)
            } else {
                mapped.with_body_field(field)
            };
        }
        mapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_consume_the_shared_saved_search_fixture_without_rest_field_changes() {
        let data: CreateSearchFilterData = serde_json::from_str(include_str!(
            "../../../aura-historia-contracts/tests/fixtures/saved-search.json"
        ))
        .unwrap();
        let command = data.into_command(Default::default()).unwrap();
        assert!(command.notifications);
        assert_eq!(localization::Language::De, command.search.language);
        assert_eq!(
            Some(u64::MAX),
            command.search.price_query.unwrap().max.map(u64::from)
        );
    }
}
