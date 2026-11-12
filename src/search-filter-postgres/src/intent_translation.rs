//! Storage of the internal English translation of a filter's semantic intent, kept beside the
//! filter's embedding and never in its user-visible `search`.

use crate::mapping::product_search_from_json;
use application::error::box_error;
use domain_primitives::query::text_query::TextQuery;
use product_listing_core::product_listing_search::EnhancedSearchDescription;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::{EnglishSearchIntent, SearchIntentFingerprint, SearchIntentTranslation};
use search_filter_service::ports::{
    SearchIntentTranslationRecordOutcome, SearchIntentTranslationStore,
    SearchIntentTranslationStoreError, UntranslatedSearchIntent,
    UntranslatedSearchIntentPageRequest,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;

/// `english` is null when the source text is English already.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentTranslationJson {
    source_fingerprint: String,
    english: Option<EnglishIntentJson>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnglishIntentJson {
    product_queries: Vec<String>,
    enhanced_search_description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IntentTranslationMappingError {
    #[error("persisted search intent translation does not match its schema")]
    Deserialize(#[source] serde_json::Error),
    #[error("persisted search intent fingerprint is not a lowercase SHA-256 hex digest")]
    Fingerprint,
    #[error("persisted search intent translation is not in canonical form")]
    NonCanonicalText,
}

impl IntentTranslationMappingError {
    /// A loggable label; the deserialization error itself may quote persisted text.
    pub(crate) const fn category(&self) -> &'static str {
        match self {
            Self::Deserialize(_) => "schema",
            Self::Fingerprint => "fingerprint",
            Self::NonCanonicalText => "non_canonical_text",
        }
    }
}

pub(crate) fn intent_translation_to_json(
    translation: &SearchIntentTranslation,
) -> serde_json::Value {
    let json = IntentTranslationJson {
        source_fingerprint: hex(translation.source().as_bytes()),
        english: translation
            .english_intent()
            .map(|english| EnglishIntentJson {
                product_queries: english
                    .product_queries()
                    .iter()
                    .map(|query| query.as_ref().to_owned())
                    .collect(),
                enhanced_search_description: english
                    .enhanced_search_description()
                    .map(|description| description.as_ref().to_owned()),
            }),
    };
    serde_json::to_value(json).unwrap_or_else(|_| unreachable!("string-only JSON serializes"))
}

/// Fails closed: text a newtype would trim, truncate or reject is invalid state, not a translation.
pub(crate) fn intent_translation_from_json(
    value: serde_json::Value,
) -> Result<SearchIntentTranslation, IntentTranslationMappingError> {
    let json: IntentTranslationJson =
        serde_json::from_value(value).map_err(IntentTranslationMappingError::Deserialize)?;
    let source = SearchIntentFingerprint::from_bytes(
        parse_hex(&json.source_fingerprint).ok_or(IntentTranslationMappingError::Fingerprint)?,
    );
    let english = json
        .english
        .map(|english| {
            let product_queries = english
                .product_queries
                .iter()
                .map(|query| canonical(query, TextQuery::<1>::try_from(query.as_str()).ok()))
                .collect::<Result<Vec<_>, _>>()?;
            let enhanced_search_description = english
                .enhanced_search_description
                .as_deref()
                .map(|text| canonical(text, EnhancedSearchDescription::try_from(text).ok()))
                .transpose()?;
            Ok(EnglishSearchIntent::new(
                product_queries,
                enhanced_search_description,
            ))
        })
        .transpose()?;
    Ok(SearchIntentTranslation::rehydrate(source, english))
}

fn canonical<T: AsRef<str>>(
    persisted: &str,
    parsed: Option<T>,
) -> Result<T, IntentTranslationMappingError> {
    parsed
        .filter(|value| value.as_ref() == persisted)
        .ok_or(IntentTranslationMappingError::NonCanonicalText)
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

#[derive(Clone)]
pub struct SqlxSearchIntentTranslationStore {
    pool: PgPool,
}

impl SqlxSearchIntentTranslationStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct UntranslatedRow {
    user_search_filter_id: uuid::Uuid,
    version: i64,
    search: serde_json::Value,
}

#[async_trait::async_trait]
impl SearchIntentTranslationStore for SqlxSearchIntentTranslationStore {
    async fn find_untranslated_page(
        &self,
        request: UntranslatedSearchIntentPageRequest,
    ) -> Result<Vec<UntranslatedSearchIntent>, SearchIntentTranslationStoreError> {
        let limit = i64::try_from(request.page_size).map_err(|source| {
            SearchIntentTranslationStoreError::ReadFailed {
                source: box_error(source),
            }
        })?;
        sqlx::query_as::<_, UntranslatedRow>(
            r#"
            SELECT user_search_filter_id, version, search
            FROM search_filters
            WHERE state = 'ACTIVE' AND intent_translation IS NULL
                AND (enhanced_search_description IS NOT NULL OR EXISTS (
                    SELECT 1 FROM jsonb_array_elements_text(search->'product_listing_query') AS query(text)
                    WHERE btrim(query.text) <> ''
                ))
                AND (intent_translation_attempted IS NULL OR intent_translation_attempted < $1)
            ORDER BY intent_translation_attempted ASC NULLS FIRST, user_search_filter_id ASC
            LIMIT $2
            "#,
        )
        .bind(request.attempted_before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| SearchIntentTranslationStoreError::ReadFailed {
            source: box_error(source),
        })?
        .into_iter()
        .map(|row| {
            let invalid = |source| SearchIntentTranslationStoreError::InvalidPersistedState {
                source,
            };
            Ok(UntranslatedSearchIntent {
                search_filter_id: UserSearchFilterId::try_from(row.user_search_filter_id)
                    .map_err(|source| invalid(box_error(source)))?,
                version: row.version,
                search: product_search_from_json(row.search)
                    .map_err(|source| invalid(box_error(source)))?,
            })
        })
        .collect()
    }

    async fn record(
        &self,
        search_filter_id: UserSearchFilterId,
        expected_version: i64,
        translation: &SearchIntentTranslation,
    ) -> Result<SearchIntentTranslationRecordOutcome, SearchIntentTranslationStoreError> {
        // Internal state: neither `version` nor `updated` advances, so a frozen run stays valid.
        let result = sqlx::query(
            "UPDATE search_filters SET intent_translation = $3 WHERE user_search_filter_id = $1 AND version = $2 AND intent_translation IS NULL",
        )
        .bind(search_filter_id.into_uuid())
        .bind(expected_version)
        .bind(intent_translation_to_json(translation))
        .execute(&self.pool)
        .await
        .map_err(|source| SearchIntentTranslationStoreError::WriteFailed {
            source: box_error(source),
        })?;
        Ok(if result.rows_affected() == 1 {
            SearchIntentTranslationRecordOutcome::Recorded
        } else {
            SearchIntentTranslationRecordOutcome::FilterChanged
        })
    }

    async fn record_failed_attempt(
        &self,
        search_filter_id: UserSearchFilterId,
        attempted_at: OffsetDateTime,
    ) -> Result<(), SearchIntentTranslationStoreError> {
        sqlx::query(
            "UPDATE search_filters SET intent_translation_attempted = $2 WHERE user_search_filter_id = $1",
        )
        .bind(search_filter_id.into_uuid())
        .bind(attempted_at)
        .execute(&self.pool)
        .await
        .map_err(|source| SearchIntentTranslationStoreError::WriteFailed {
            source: box_error(source),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_listing_search::ProductListingSearch;

    fn search() -> ProductListingSearch {
        ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query("Manschettenknöpfe".try_into().unwrap())
            .with_product_listing_query("Broschen".try_into().unwrap())
            .with_enhanced_search_description("mit rotem Stein".try_into().unwrap())
    }

    fn translation() -> SearchIntentTranslation {
        SearchIntentTranslation::english(
            &search(),
            EnglishSearchIntent::new(
                vec![
                    "cufflinks".try_into().unwrap(),
                    "brooches".try_into().unwrap(),
                ],
                Some("with a red stone".try_into().unwrap()),
            ),
        )
        .unwrap()
    }

    #[test]
    fn should_round_trip_translations_and_the_already_english_marker() {
        for translation in [
            translation(),
            SearchIntentTranslation::already_english(&search()),
        ] {
            let json = intent_translation_to_json(&translation);

            assert_eq!(translation, intent_translation_from_json(json).unwrap());
        }
    }

    #[test]
    fn should_persist_the_documented_shape() {
        let json = intent_translation_to_json(&translation());

        assert_eq!(
            serde_json::json!(["cufflinks", "brooches"]),
            json["english"]["product_queries"]
        );
        assert_eq!(
            "with a red stone",
            json["english"]["enhanced_search_description"]
        );
        assert_eq!(64, json["source_fingerprint"].as_str().unwrap().len());
        let marker =
            intent_translation_to_json(&SearchIntentTranslation::already_english(&search()));
        assert!(marker["english"].is_null());
    }

    #[test]
    fn should_fail_closed_on_invalid_persisted_translations() {
        let valid = intent_translation_to_json(&translation());
        let mutations: [fn(&mut serde_json::Value); 7] = [
            |json| json["source_fingerprint"] = "abc".into(),
            |json| json["source_fingerprint"] = "G".repeat(64).into(),
            |json| json["source_fingerprint"] = "A".repeat(64).into(),
            |json| json["english"]["product_queries"][0] = "".into(),
            |json| json["english"]["product_queries"][0] = "x".repeat(300).into(),
            |json| json["english"]["enhanced_search_description"] = "   ".into(),
            |json| json["unexpected"] = true.into(),
        ];
        for mutate in mutations {
            let mut json = valid.clone();
            mutate(&mut json);

            assert!(
                intent_translation_from_json(json.clone()).is_err(),
                "{json}"
            );
        }
    }
}
