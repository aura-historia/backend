//! Saved-search match previews for REST. They read the classifier model and acceptance
//! threshold deployed for scheduled matching, so a preview accepts what a scheduled run would
//! accept. Previews always classify single candidates, so scheduled matching's batch size does
//! not apply. MCP does not expose previews.

use crate::ProductListingDependencies;
use classifier_model::{
    ClassifierModel, CloudflareClassifierConfig, CloudflareClassifierModel, CloudflareModel,
    Probability,
};
use fxrate_postgres::SqlxFxRateSnapshotReader;
use platform_postgres::SqlxUnitOfWork;
use product_listing_opensearch::OpenSearchProductListingSearchReader;
use product_listing_postgres::{
    SqlxListingSourceSummaryReader, SqlxProductListingContentAssessmentReader,
    SqlxProductListingSearchFilterMatchSourceReaderFactory, SqlxProductListingUserStateReader,
};
use product_listing_service::shipping::ShippingEnrichment;
use product_listing_service::use_cases::ProductListingSearchPresentation;
use search_filter_service::match_classification::{
    MatchClassificationSettingsError, should_show_threshold,
};
use search_filter_service::ports::SearchIntentTranslator;
use search_filter_service::use_cases::{
    PreviewSearchFilterMatchesHandler, PreviewSearchFilterMatchesUseCase,
    SEARCH_FILTER_MATCH_PREVIEW_CLASSIFICATION_DEADLINE,
    SEARCH_FILTER_MATCH_PREVIEW_IMAGE_FETCH_DEADLINE, SearchFilterMatchPreviewPolicy,
};
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::Arc;

pub const CLASSIFIER_MODEL_PROVIDER_ENV: &str = "CLASSIFIER_MODEL_PROVIDER";
pub const CLASSIFIER_MODEL_ENV: &str = "CLASSIFIER_MODEL";
pub const CLOUDFLARE_ACCOUNT_ID_ENV: &str = "CLOUDFLARE_ACCOUNT_ID";
pub const CLOUDFLARE_API_TOKEN_ENV: &str = "CLOUDFLARE_API_TOKEN";
pub const SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS_ENV: &str =
    "SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS";
const CLOUDFLARE_PROVIDER: &str = "cloudflare";
const DEFAULT_SHOULD_SHOW_THRESHOLD_BPS: u16 = 5_000;

/// Deliberately not `Debug`: it holds the classifier API token.
#[derive(Clone, PartialEq, Eq)]
pub struct SavedSearchPreviewConfig {
    classifier: CloudflareClassifierConfig,
    should_show_threshold: Probability,
}

#[derive(Debug, thiserror::Error)]
pub enum SavedSearchPreviewConfigError {
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("{CLASSIFIER_MODEL_PROVIDER_ENV} must be {CLOUDFLARE_PROVIDER}")]
    UnsupportedClassifierProvider,
    #[error("{CLASSIFIER_MODEL_ENV} names no supported classifier model")]
    UnsupportedClassifierModel,
    #[error("Cloudflare classifier account or token is invalid")]
    InvalidClassifierCredentials,
    #[error("invalid numeric environment variable {name}")]
    InvalidNumber {
        name: &'static str,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("invalid saved-search acceptance threshold")]
    InvalidThreshold(#[source] MatchClassificationSettingsError),
}

impl SavedSearchPreviewConfig {
    pub fn from_getter<F>(mut get: F) -> Result<Self, SavedSearchPreviewConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let mut value = |name| get(name).map(|value| value.trim().to_owned());
        let provider = value(CLASSIFIER_MODEL_PROVIDER_ENV);
        if provider
            .as_deref()
            .is_some_and(|provider| provider != CLOUDFLARE_PROVIDER)
        {
            return Err(SavedSearchPreviewConfigError::UnsupportedClassifierProvider);
        }
        let model = match value(CLASSIFIER_MODEL_ENV) {
            Some(model) => CloudflareModel::from_str(&model)
                .map_err(|_| SavedSearchPreviewConfigError::UnsupportedClassifierModel)?,
            None => CloudflareModel::default(),
        };
        let mut required = |name| {
            value(name)
                .filter(|value| !value.is_empty())
                .ok_or(SavedSearchPreviewConfigError::MissingEnv { name })
        };
        let account_id = required(CLOUDFLARE_ACCOUNT_ID_ENV)?;
        let api_token = required(CLOUDFLARE_API_TOKEN_ENV)?;
        let classifier = CloudflareClassifierConfig::new(account_id, api_token, model)
            .map_err(|_| SavedSearchPreviewConfigError::InvalidClassifierCredentials)?;
        let threshold_basis_points = match value(SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS_ENV)
        {
            Some(raw) => {
                raw.parse()
                    .map_err(|source| SavedSearchPreviewConfigError::InvalidNumber {
                        name: SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS_ENV,
                        source,
                    })?
            }
            None => DEFAULT_SHOULD_SHOW_THRESHOLD_BPS,
        };
        let should_show_threshold = should_show_threshold(threshold_basis_points)
            .map_err(SavedSearchPreviewConfigError::InvalidThreshold)?;
        Ok(Self {
            classifier,
            should_show_threshold,
        })
    }
}

/// Providers built once per process from [`SavedSearchPreviewConfig`].
pub struct SavedSearchPreviewDependencies {
    pub classifier: Arc<dyn ClassifierModel>,
    pub should_show_threshold: Probability,
    /// Translates the unsaved intent, as a filter write would.
    pub translator: Arc<dyn SearchIntentTranslator>,
}

impl SavedSearchPreviewDependencies {
    pub fn from_config(
        config: &SavedSearchPreviewConfig,
        translator: Arc<dyn SearchIntentTranslator>,
    ) -> Result<Self, SavedSearchPreviewConfigError> {
        let classifier = CloudflareClassifierModel::new(config.classifier.clone())
            .map_err(|_| SavedSearchPreviewConfigError::InvalidClassifierCredentials)?
            .with_image_fetch_deadline(SEARCH_FILTER_MATCH_PREVIEW_IMAGE_FETCH_DEADLINE);
        Ok(Self {
            classifier: Arc::new(classifier),
            should_show_threshold: config.should_show_threshold,
            translator,
        })
    }
}

#[derive(Clone)]
pub struct SavedSearchPreviews {
    pub preview: Arc<dyn PreviewSearchFilterMatchesUseCase>,
}

impl SavedSearchPreviews {
    pub(super) fn compose(
        pool: &PgPool,
        shipping: ShippingEnrichment,
        listing: &ProductListingDependencies,
        dependencies: SavedSearchPreviewDependencies,
    ) -> Self {
        let presentation = ProductListingSearchPresentation::new(
            SqlxListingSourceSummaryReader::new(pool.clone()),
            SqlxProductListingUserStateReader::new(pool.clone()),
            SqlxProductListingContentAssessmentReader::new(pool.clone()),
            shipping,
        )
        .with_read_execution_policy(listing.read_execution_policy);
        Self {
            preview: Arc::new(PreviewSearchFilterMatchesHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                user_postgres::SqlxUserTierEntitlementsFactory::new(),
                Arc::clone(&listing.embeddings),
                dependencies.translator,
                SqlxFxRateSnapshotReader::new(pool.clone()),
                OpenSearchProductListingSearchReader::new(listing.opensearch.clone()),
                SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
                dependencies.classifier,
                presentation,
                SearchFilterMatchPreviewPolicy {
                    should_show_threshold: dependencies.should_show_threshold,
                    classification_deadline: SEARCH_FILTER_MATCH_PREVIEW_CLASSIFICATION_DEADLINE,
                },
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(
        overrides: &[(&'static str, Option<&'static str>)],
    ) -> Result<SavedSearchPreviewConfig, SavedSearchPreviewConfigError> {
        let mut values = HashMap::from([
            (CLOUDFLARE_ACCOUNT_ID_ENV, "fixture-account"),
            (CLOUDFLARE_API_TOKEN_ENV, "fixture-token"),
        ]);
        for (name, value) in overrides {
            match value {
                Some(value) => values.insert(name, value),
                None => values.remove(name),
            };
        }
        SavedSearchPreviewConfig::from_getter(|name| {
            values.get(name).map(|value| (*value).to_owned())
        })
    }

    #[test]
    fn should_default_to_the_scheduled_matching_classifier_settings() {
        let config = config(&[]).unwrap();

        assert_eq!(CloudflareModel::ClefFlash, config.classifier.model());
        assert_eq!(0.5, config.should_show_threshold.get());
    }

    #[test]
    fn should_read_the_deployed_model_and_threshold() {
        let config = config(&[
            (CLASSIFIER_MODEL_PROVIDER_ENV, Some("cloudflare")),
            (CLASSIFIER_MODEL_ENV, Some(" clef ")),
            (
                SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS_ENV,
                Some("6500"),
            ),
        ])
        .unwrap();

        assert_eq!(CloudflareModel::Clef, config.classifier.model());
        assert_eq!(0.65, config.should_show_threshold.get());
    }

    #[test]
    fn should_refuse_to_start_without_a_usable_classifier() {
        for (overrides, expected) in [
            (vec![(CLOUDFLARE_API_TOKEN_ENV, None)], "missing"),
            (vec![(CLOUDFLARE_ACCOUNT_ID_ENV, Some("  "))], "missing"),
            (
                vec![(CLASSIFIER_MODEL_PROVIDER_ENV, Some("openai"))],
                "provider",
            ),
            (vec![(CLASSIFIER_MODEL_ENV, Some("gemini"))], "model"),
            (
                vec![(CLOUDFLARE_API_TOKEN_ENV, Some("bad token"))],
                "credentials",
            ),
            (
                vec![(
                    SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS_ENV,
                    Some("10001"),
                )],
                "threshold",
            ),
            (
                vec![(
                    SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS_ENV,
                    Some("0.5"),
                )],
                "number",
            ),
        ] {
            let actual = match config(&overrides) {
                Err(SavedSearchPreviewConfigError::MissingEnv { .. }) => "missing",
                Err(SavedSearchPreviewConfigError::UnsupportedClassifierProvider) => "provider",
                Err(SavedSearchPreviewConfigError::UnsupportedClassifierModel) => "model",
                Err(SavedSearchPreviewConfigError::InvalidClassifierCredentials) => "credentials",
                Err(SavedSearchPreviewConfigError::InvalidThreshold(_)) => "threshold",
                Err(SavedSearchPreviewConfigError::InvalidNumber { .. }) => "number",
                Ok(_) => "accepted",
            };
            assert_eq!(expected, actual, "{overrides:?}");
        }
    }
}
