use crate::ports::ProductListingTitleTranslationError;
use indexmap::IndexMap;
use localization::Language;
use product_listing_core::title::Title;

/// Titles of one source language that one provider request translates.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleTranslationGroup {
    /// Unique within its batch.
    pub group: u32,
    pub source_language: Language,
    pub target_languages: Vec<Language>,
    pub titles: Vec<PositionedTitle>,
}

/// A title and its identity within its group, unique and positive.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedTitle {
    pub position: u32,
    pub title: Title,
}

/// The provider's handle of a submitted batch, as the translator returned it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TitleTranslationBatchId(String);

#[derive(Debug, thiserror::Error)]
#[error("title translation batch ID is invalid")]
pub struct InvalidTitleTranslationBatchId;

impl TitleTranslationBatchId {
    const MAX_CHARS: usize = 255;

    pub fn new(value: impl Into<String>) -> Result<Self, InvalidTitleTranslationBatchId> {
        let value = value.into();
        if value.is_empty()
            || value.len() > Self::MAX_CHARS
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(InvalidTitleTranslationBatchId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TitleTranslationBatchId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug)]
pub enum TitleTranslationBatchPoll {
    InProgress,
    /// Results of the groups the provider returned; a group it did not return was not processed.
    Ended(Vec<TitleTranslationGroupResult>),
}

#[derive(Debug)]
pub struct TitleTranslationGroupResult {
    pub group: u32,
    pub outcome: TitleTranslationGroupOutcome,
}

/// One title's translations by target language, or why its answer was rejected.
pub type TitleTranslationResult =
    Result<IndexMap<Language, Title>, ProductListingTitleTranslationError>;

#[derive(Debug)]
pub enum TitleTranslationGroupOutcome {
    /// One result per position of the polled group. A title the answer omitted or translated
    /// implausibly fails alone.
    Translated(Vec<(u32, TitleTranslationResult)>),
    /// The model declined the request. Asked alone, or of another model, a title may succeed.
    Refused,
    /// The answer outgrew its output budget. Asked alone, a title may succeed.
    Truncated,
    /// The request failed for every title of the group.
    Failed(ProductListingTitleTranslationError),
    /// The batch expired or was canceled before the request was processed.
    NotProcessed,
}

/// Translates title groups asynchronously through a provider batch.
#[async_trait::async_trait]
pub trait ProductListingTitleBatchTranslator: Send + Sync {
    /// Splits `titles` of one source language, in order, into the requests that translate them:
    /// consecutive group sizes that sum to `titles.len()`.
    fn group_sizes(
        &self,
        source_language: Language,
        target_languages: &[Language],
        titles: &[Title],
    ) -> Vec<usize>;

    async fn submit(
        &self,
        groups: &[TitleTranslationGroup],
    ) -> Result<TitleTranslationBatchId, ProductListingTitleTranslationError>;

    /// `groups` are the groups the batch was submitted with, as far as they are still current.
    async fn poll(
        &self,
        batch: &TitleTranslationBatchId,
        groups: &[TitleTranslationGroup],
    ) -> Result<TitleTranslationBatchPoll, ProductListingTitleTranslationError>;
}
