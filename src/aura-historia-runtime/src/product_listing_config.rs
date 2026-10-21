//! Configuration required only by listing discovery; no unrelated provider settings are read.
use embedding::VertexAiEmbeddingConfig;
use fxrate_service::readers::FxSearchCacheConfig;
use product_listing_service::{
    readers::SourceSearchCacheConfig, use_cases::ProductListingSearchReadExecutionPolicy,
};
use std::time::Duration;

pub const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
pub const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
pub const PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV: &str =
    "PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED";
pub const PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED_ENV: &str =
    "PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED";
pub const PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES_ENV: &str =
    "PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES";
pub const PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS_ENV: &str =
    "PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS";
pub const PRODUCT_LISTING_SEARCH_SOURCE_CACHE_ENABLED_ENV: &str =
    "PRODUCT_LISTING_SEARCH_SOURCE_CACHE_ENABLED";
pub const PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES_ENV: &str =
    "PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES";
pub const PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES_ENV: &str =
    "PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES";
pub const PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS_ENV: &str =
    "PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS";
const DEFAULT_PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES: u64 = 512;
const DEFAULT_PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS: u64 = 30;
const DEFAULT_PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES: u64 = 4_096;
const DEFAULT_PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS: u64 = 60;
const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "project-2c6e1dcc-3fb9-4910-adc";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";

#[derive(Clone, PartialEq, Eq)]
pub struct ProductListingConfig {
    vertex_ai_embedding: VertexAiEmbeddingConfig,
    fx_cache: FxSearchCacheConfig,
    source_cache: SourceSearchCacheConfig,
    read_execution_policy: ProductListingSearchReadExecutionPolicy,
}

impl ProductListingConfig {
    pub fn from_env() -> Result<Self, ProductListingConfigError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, ProductListingConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let vertex_ai_embedding = VertexAiEmbeddingConfig::new(
            get(VERTEX_AI_PROJECT_ID_ENV)
                .unwrap_or_else(|| DEFAULT_VERTEX_AI_PROJECT_ID.to_owned()),
            get(VERTEX_AI_LOCATION_ENV).unwrap_or_else(|| DEFAULT_VERTEX_AI_LOCATION.to_owned()),
        );
        let parallel = optional_bool_config(
            &mut get,
            PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV,
            false,
        )?;
        Ok(Self {
            vertex_ai_embedding,
            fx_cache: parse_product_listing_search_fx_cache_config(&mut get)?,
            source_cache: parse_product_listing_search_source_cache_config(&mut get)?,
            read_execution_policy: if parallel {
                ProductListingSearchReadExecutionPolicy::Concurrent
            } else {
                ProductListingSearchReadExecutionPolicy::Sequential
            },
        })
    }

    pub fn vertex_ai_embedding(&self) -> &VertexAiEmbeddingConfig {
        &self.vertex_ai_embedding
    }
    pub fn fx_cache(&self) -> FxSearchCacheConfig {
        self.fx_cache.clone()
    }
    pub fn source_cache(&self) -> SourceSearchCacheConfig {
        self.source_cache.clone()
    }
    pub fn read_execution_policy(&self) -> ProductListingSearchReadExecutionPolicy {
        self.read_execution_policy
    }
}

fn optional_bool_config<F>(
    get: &mut F,
    name: &'static str,
    default: bool,
) -> Result<bool, ProductListingConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match get(name) {
        Some(value) => value
            .parse()
            .map_err(|_| ProductListingConfigError::InvalidBooleanConfig { name, value }),
        None => Ok(default),
    }
}

fn optional_bounded_u64_config<F>(
    get: &mut F,
    name: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ProductListingConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let Some(value) = get(name) else {
        return Ok(default);
    };
    let parsed =
        value
            .parse::<u64>()
            .map_err(|_| ProductListingConfigError::InvalidIntegerConfig {
                name,
                value: value.clone(),
            })?;
    if !(min..=max).contains(&parsed) {
        return Err(ProductListingConfigError::OutOfRangeIntegerConfig {
            name,
            value,
            min,
            max,
        });
    }
    Ok(parsed)
}

fn optional_bounded_usize_config<F>(
    get: &mut F,
    name: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<usize, ProductListingConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let value = optional_bounded_u64_config(get, name, default, min, max)?;
    usize::try_from(value).map_err(|_| ProductListingConfigError::OutOfRangeIntegerConfig {
        name,
        value: value.to_string(),
        min,
        max,
    })
}

fn parse_product_listing_search_fx_cache_config<F>(
    get: &mut F,
) -> Result<FxSearchCacheConfig, ProductListingConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let enabled = optional_bool_config(get, PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED_ENV, true)?;
    let max_entries = optional_bounded_usize_config(
        get,
        PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES_ENV,
        DEFAULT_PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES,
        1,
        8_192,
    )?;
    let latest_ttl_seconds = optional_bounded_u64_config(
        get,
        PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS_ENV,
        DEFAULT_PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS,
        0,
        300,
    )?;

    FxSearchCacheConfig::new(
        enabled,
        max_entries,
        Duration::from_secs(latest_ttl_seconds),
    )
    .map_err(ProductListingConfigError::FxSearchCacheConfig)
}

fn parse_product_listing_search_source_cache_config<F>(
    get: &mut F,
) -> Result<SourceSearchCacheConfig, ProductListingConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let enabled = optional_bool_config(get, PRODUCT_LISTING_SEARCH_SOURCE_CACHE_ENABLED_ENV, true)?;
    let max_entries = optional_bounded_usize_config(
        get,
        PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES_ENV,
        DEFAULT_PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES,
        1,
        65_536,
    )?;
    let max_accounted_bytes = optional_bounded_usize_config(
        get,
        PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES_ENV,
        DEFAULT_PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES,
        1,
        536_870_912,
    )?;
    let ttl_seconds = optional_bounded_u64_config(
        get,
        PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS_ENV,
        DEFAULT_PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS,
        1,
        300,
    )?;

    SourceSearchCacheConfig::new(
        enabled,
        max_entries,
        max_accounted_bytes,
        Duration::from_secs(ttl_seconds),
    )
    .map_err(ProductListingConfigError::SourceSearchCacheConfig)
}

#[derive(thiserror::Error, Debug)]
pub enum ProductListingConfigError {
    #[error("invalid boolean configuration {name}: {value}")]
    InvalidBooleanConfig { name: &'static str, value: String },
    #[error("invalid integer configuration {name}: {value}")]
    InvalidIntegerConfig { name: &'static str, value: String },
    #[error("integer configuration {name} must be between {min} and {max}: {value}")]
    OutOfRangeIntegerConfig {
        name: &'static str,
        value: String,
        min: u64,
        max: u64,
    },
    #[error("invalid product listing search FX cache configuration")]
    FxSearchCacheConfig(#[source] fxrate_service::readers::FxSearchCacheConfigError),
    #[error("invalid product listing search source cache configuration")]
    SourceSearchCacheConfig(
        #[source] product_listing_service::readers::SourceSearchCacheConfigError,
    ),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_select_only_listing_settings_and_preserve_the_existing_defaults() {
        let config = ProductListingConfig::from_getter(|name| {
            assert!(
                name.starts_with("PRODUCT_LISTING_SEARCH_") || name.starts_with("VERTEX_AI_"),
                "unrelated setting {name}"
            );
            None
        })
        .unwrap();
        assert!(
            config.vertex_ai_embedding()
                == &VertexAiEmbeddingConfig::new(
                    DEFAULT_VERTEX_AI_PROJECT_ID,
                    DEFAULT_VERTEX_AI_LOCATION
                )
        );
        assert_eq!(
            ProductListingSearchReadExecutionPolicy::Sequential,
            config.read_execution_policy()
        );
        assert!(config.fx_cache().enabled());
        assert!(config.source_cache().enabled());
    }

    #[test]
    fn should_apply_explicit_vertex_and_parallel_read_configuration() {
        let config = ProductListingConfig::from_getter(|name| match name {
            VERTEX_AI_PROJECT_ID_ENV => Some("fixture-project".to_owned()),
            VERTEX_AI_LOCATION_ENV => Some("fixture-region".to_owned()),
            PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV => Some("true".to_owned()),
            _ => None,
        })
        .unwrap();
        assert!(
            config.vertex_ai_embedding()
                == &VertexAiEmbeddingConfig::new("fixture-project", "fixture-region")
        );
        assert_eq!(
            ProductListingSearchReadExecutionPolicy::Concurrent,
            config.read_execution_policy()
        );
    }
    #[test]
    fn should_default_parallel_product_listing_enrichment_to_disabled() {
        let mut get = |_| None;

        let enabled = optional_bool_config(
            &mut get,
            PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV,
            false,
        );

        assert!(matches!(enabled, Ok(false)));
    }

    #[test]
    fn should_default_product_listing_search_cache_policies_to_enabled() {
        let mut get = |_| None;

        let fx = parse_product_listing_search_fx_cache_config(&mut get);
        let source = parse_product_listing_search_source_cache_config(&mut get);

        assert!(matches!(
            fx,
            Ok(ref config)
                if config.enabled()
                    && config.max_entries() == 512
                    && config.latest_selection_ttl() == Duration::from_secs(30)
        ));
        assert!(matches!(
            source,
            Ok(ref config)
                if config.enabled()
                    && config.max_entries() == 4_096
                    && config.max_accounted_bytes() == 8 * 1024 * 1024
                    && config.ttl() == Duration::from_secs(60)
        ));
    }

    #[test]
    fn should_apply_explicit_product_listing_search_cache_policy() {
        let mut get = |name| match name {
            PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED_ENV => Some("false".to_owned()),
            PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES_ENV => Some("128".to_owned()),
            PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS_ENV => Some("0".to_owned()),
            PRODUCT_LISTING_SEARCH_SOURCE_CACHE_ENABLED_ENV => Some("false".to_owned()),
            PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES_ENV => Some("256".to_owned()),
            PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES_ENV => Some("65536".to_owned()),
            PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS_ENV => Some("5".to_owned()),
            _ => None,
        };

        let fx = parse_product_listing_search_fx_cache_config(&mut get);
        let source = parse_product_listing_search_source_cache_config(&mut get);

        assert!(matches!(
            fx,
            Ok(ref config)
                if !config.enabled()
                    && config.max_entries() == 128
                    && config.latest_selection_ttl().is_zero()
        ));
        assert!(matches!(
            source,
            Ok(ref config)
                if !config.enabled()
                    && config.max_entries() == 256
                    && config.max_accounted_bytes() == 65_536
                    && config.ttl() == Duration::from_secs(5)
        ));
    }

    #[test]
    fn should_reject_out_of_range_product_listing_search_cache_configuration() {
        let mut get = |name| {
            (name == PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES_ENV)
                .then(|| "536870913".to_owned())
        };

        let result = parse_product_listing_search_source_cache_config(&mut get);

        assert!(matches!(
            result,
            Err(ProductListingConfigError::OutOfRangeIntegerConfig { name, value, min: 1, max: 536_870_912 })
                if name == PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES_ENV && value == "536870913"
        ));
    }

    #[test]
    fn should_reject_non_integer_product_listing_search_cache_configuration() {
        let mut get = |name| {
            (name == PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES_ENV).then(|| "many".to_owned())
        };

        let result = parse_product_listing_search_fx_cache_config(&mut get);

        assert!(matches!(
            result,
            Err(ProductListingConfigError::InvalidIntegerConfig { name, value })
                if name == PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES_ENV && value == "many"
        ));
    }

    #[test]
    fn should_reject_non_boolean_product_listing_search_fx_cache_configuration() {
        let mut get = |name| {
            (name == PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED_ENV).then(|| "enabled".to_owned())
        };

        let result =
            optional_bool_config(&mut get, PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED_ENV, false);

        assert!(matches!(
            result,
            Err(ProductListingConfigError::InvalidBooleanConfig { name, value })
                if name == PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED_ENV && value == "enabled"
        ));
    }

    #[test]
    fn should_reject_non_boolean_parallel_product_listing_enrichment_configuration() {
        let mut get = |name| {
            (name == PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV)
                .then(|| "enabled".to_owned())
        };

        let result = optional_bool_config(
            &mut get,
            PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV,
            false,
        );

        assert!(matches!(
            result,
            Err(ProductListingConfigError::InvalidBooleanConfig { name, value })
                if name == PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV && value == "enabled"
        ));
    }
}
