//! Address suggestions for search-as-you-type address entry, and the `placeId` a picked
//! suggestion contributes to address and Party-location writes.
use geo_service::{
    geocoding::PlaceReference,
    suggestions::{AddressSuggestion, AddressSuggestions, SuggestionQuery},
};
use localization::Language;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

pub(crate) fn place_id_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string", "minLength": 1, "maxLength": PlaceReference::MAX_BYTES,
        "pattern": "^\\S+$",
        "description": "Opaque place reference from an address suggestion, passed back unchanged."
    })
}

/// Validates an optional `placeId` through its constructor.
pub(crate) fn place_reference(
    place_id: Option<String>,
) -> Result<Option<PlaceReference>, InvalidSuggestionData> {
    place_id
        .map(|id| PlaceReference::new(id).map_err(|_| InvalidSuggestionData))
        .transpose()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid address suggestion input")]
pub struct InvalidSuggestionData;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestData {
    /// Partly typed address: 3 to 200 characters after trimming, without control characters.
    #[schemars(length(min = 3, max = 200))]
    pub query: String,
    /// Language to render suggestions in, where the provider has it.
    #[serde(with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    pub language: Language,
}

impl SuggestData {
    pub fn into_request(self) -> Result<(SuggestionQuery, Language), InvalidSuggestionData> {
        let query = SuggestionQuery::new(&self.query).map_err(|_| InvalidSuggestionData)?;
        Ok((query, self.language))
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionData {
    /// Pass back as `placeId` when saving the address this suggestion fills in.
    #[schemars(schema_with = "place_id_schema")]
    pub place_id: String,
    /// Display text for the suggestion.
    pub label: String,
    /// `ADDRESS` is a complete address; other kinds still need details such as a house number.
    #[schemars(schema_with = "kind_schema")]
    pub kind: &'static str,
}

fn kind_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "string", "enum": ["ADDRESS", "STREET", "LOCALITY", "POSTAL_CODE", "REGION", "OTHER"]})
}

impl From<AddressSuggestion> for SuggestionData {
    fn from(v: AddressSuggestion) -> Self {
        Self {
            place_id: v.place.as_str().to_owned(),
            label: v.label,
            kind: v.kind.as_str(),
        }
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionAttributionData {
    pub provider: &'static str,
    pub attribution_url: &'static str,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionsData {
    /// At most five suggestions, best first.
    pub suggestions: Vec<SuggestionData>,
    /// Show this attribution with the suggestions.
    pub attribution: SuggestionAttributionData,
}

impl From<AddressSuggestions> for SuggestionsData {
    fn from(v: AddressSuggestions) -> Self {
        Self {
            suggestions: v.suggestions.into_iter().map(Into::into).collect(),
            attribution: SuggestionAttributionData {
                provider: v.attribution.provider,
                attribution_url: v.attribution.attribution_url,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_accept_only_query_and_language_when_requesting_suggestions() {
        let data: SuggestData =
            serde_json::from_value(json!({"query": "  Haupt  ", "language": "de"})).unwrap();
        let (query, language) = data.into_request().unwrap();
        assert_eq!(("Haupt", Language::De), (query.as_str(), language));
        for invalid in [
            json!({"query": "Hauptstraße", "language": "de", "limit": 10}),
            json!({"query": "Hauptstraße", "language": "xx"}),
            json!({"query": "Hauptstraße"}),
        ] {
            assert!(serde_json::from_value::<SuggestData>(invalid).is_err());
        }
        let short: SuggestData =
            serde_json::from_value(json!({"query": " ab ", "language": "en"})).unwrap();
        assert_eq!(Err(InvalidSuggestionData), short.into_request().map(|_| ()));
    }

    #[test]
    fn should_validate_place_ids_through_the_reference_constructor() {
        assert_eq!(
            Some("AQAB-place"),
            place_reference(Some("AQAB-place".into()))
                .unwrap()
                .as_ref()
                .map(PlaceReference::as_str)
        );
        assert_eq!(Ok(None), place_reference(None).map(|p| p.map(|_| ())));
        assert!(place_reference(Some("two words".into())).is_err());
    }
}
