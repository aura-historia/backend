//! Typed diagnostics returned beside deterministic normalization decisions.
//!
//! Issues never change the decision they describe. They identify the evaluated input path, a
//! stable reason code, whether the failure belongs to candidate data or the normalizer itself,
//! how the decision handled the field, and bounded structured details. Details never copy
//! arbitrary input text or provider payloads.

use crate::error::{NormalizationError, NormalizationFailureScope, PriceField};
use crate::price::PriceNormalizationError;
use crate::shipping::ShippingUnsupportedReason;
use crate::{AvailabilityNormalizationError, availability::AvailabilityUnsupportedReason};
use serde::{Deserialize, Serialize};

/// Version of the issue vocabulary and evaluator diagnostics carried by logs and recorded cases.
pub const NORMALIZATION_DIAGNOSTICS_VERSION: u16 = 1;

/// Whether an issue belongs to the input envelope or to one independently evaluated field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizationStage {
    /// The input envelope or context prevented field evaluation.
    Envelope,
    /// One field was evaluated independently of the remaining fields.
    Field,
}

impl NormalizationStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Envelope => "ENVELOPE",
            Self::Field => "FIELD",
        }
    }
}

/// Evaluated input path. `path()` names the location inside the evaluated input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizationField {
    RawValuesSchemaVersion,
    RawValues,
    NormalizationContext,
    BaseUrl,
    FallbackCurrency,
    FallbackLanguage,
    SourceRecordKey,
    SourceListingId,
    Title,
    Description,
    Price,
    PriceEstimateMin,
    PriceEstimateMax,
    Availability,
    Url,
    Images,
    Shipping,
}

impl NormalizationField {
    pub const fn path(self) -> &'static str {
        match self {
            Self::RawValuesSchemaVersion => "rawValuesSchemaVersion",
            Self::RawValues => "rawValues",
            Self::NormalizationContext => "normalizationContext",
            Self::BaseUrl => "normalizationContext.baseUrl",
            Self::FallbackCurrency => "normalizationContext.fallbackCurrency",
            Self::FallbackLanguage => "normalizationContext.fallbackLanguage",
            Self::SourceRecordKey => "sourceRecordKey",
            Self::SourceListingId => "rawValues.sourceListingId",
            Self::Title => "rawValues.title",
            Self::Description => "rawValues.description",
            Self::Price => "rawValues.price",
            Self::PriceEstimateMin => "rawValues.priceEstimateMin",
            Self::PriceEstimateMax => "rawValues.priceEstimateMax",
            Self::Availability => "rawValues.availability",
            Self::Url => "rawValues.url",
            Self::Images => "rawValues.images",
            Self::Shipping => "rawValues.attributes.rawShipment",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RawValuesSchemaVersion => "RAW_VALUES_SCHEMA_VERSION",
            Self::RawValues => "RAW_VALUES",
            Self::NormalizationContext => "NORMALIZATION_CONTEXT",
            Self::BaseUrl => "BASE_URL",
            Self::FallbackCurrency => "FALLBACK_CURRENCY",
            Self::FallbackLanguage => "FALLBACK_LANGUAGE",
            Self::SourceRecordKey => "SOURCE_RECORD_KEY",
            Self::SourceListingId => "SOURCE_LISTING_ID",
            Self::Title => "TITLE",
            Self::Description => "DESCRIPTION",
            Self::Price => "PRICE",
            Self::PriceEstimateMin => "PRICE_ESTIMATE_MIN",
            Self::PriceEstimateMax => "PRICE_ESTIMATE_MAX",
            Self::Availability => "AVAILABILITY",
            Self::Url => "URL",
            Self::Images => "IMAGES",
            Self::Shipping => "SHIPPING",
        }
    }
}

impl From<PriceField> for NormalizationField {
    fn from(field: PriceField) -> Self {
        match field {
            PriceField::Price => Self::Price,
            PriceField::EstimateMin => Self::PriceEstimateMin,
            PriceField::EstimateMax => Self::PriceEstimateMax,
        }
    }
}

/// Stable reason codes. Codes are persisted and logged; never rename an existing code.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    strum_macros::EnumIter,
    strum_macros::IntoStaticStr,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizationIssueReason {
    RawValuesSchemaUnsupported,
    RawValuesInvalid,
    NormalizationContextInvalid,
    BaseUrlInvalid,
    FallbackCurrencyUnsupported,
    FallbackLanguageUnsupported,
    SourceRecordKeyInvalid,
    SourceListingIdEmpty,
    SourceListingIdInvalid,
    TitleEmpty,
    TitleUnknownLanguage,
    DescriptionUnknownLanguage,
    PriceUnknownCurrency,
    PriceParseFailed,
    PriceNegative,
    PriceExcessPrecision,
    MachineDecimalFallbackCurrencyRequired,
    AvailabilityTextTooLong,
    AvailabilityEmbeddedNul,
    AvailabilityRulesUnavailable,
    AvailabilityUnrecognized,
    AvailabilityConflictingQuantity,
    AvailabilityQualifiedQuantity,
    UrlInvalid,
    ImageUrlInvalid,
    NoValidImages,
    ShippingTooManyFragments,
    ShippingTextTooLarge,
    ShippingUnsupportedCharacter,
    ShippingConflictingPrice,
    ShippingConflictingHandling,
    ShippingInvalidHandlingRange,
    ShippingUnrecognizedFieldClause,
    ShippingUnrecognizedDeliveryClause,
    ShippingConditionalClause,
    ShippingContradictoryAssertions,
    ShippingRulesUnavailable,
    /// The resolved source listing ID differs from the identity bound to the raw stream.
    SourceListingIdBindingMismatch,
    /// Normalized values violate canonical ProductListing validation.
    CanonicalListingInvalid,
}

impl NormalizationIssueReason {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Serializable form of [`NormalizationFailureScope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizationIssueScope {
    CandidateData,
    System,
}

impl NormalizationIssueScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CandidateData => "CANDIDATE_DATA",
            Self::System => "SYSTEM",
        }
    }
}

impl From<NormalizationFailureScope> for NormalizationIssueScope {
    fn from(scope: NormalizationFailureScope) -> Self {
        match scope {
            NormalizationFailureScope::CandidateData => Self::CandidateData,
            NormalizationFailureScope::System => Self::System,
        }
    }
}

/// How the evaluated decision handled the field named by an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizationIssueHandling {
    /// The whole input (or crawler candidate) was rejected.
    RejectedInput,
    /// The supplied field was not applied; canonical state stays unchanged.
    PreservedField,
    /// The field produced no assertion inside an otherwise successful evaluation.
    UnresolvedField,
}

impl NormalizationIssueHandling {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RejectedInput => "REJECTED_INPUT",
            Self::PreservedField => "PRESERVED_FIELD",
            Self::UnresolvedField => "UNRESOLVED_FIELD",
        }
    }
}

/// Safe, bounded structured details. Never contains input text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizationIssueDetails {
    None,
    SizeLimit {
        observed: usize,
        limit: usize,
    },
    SchemaVersion {
        observed: u16,
        supported: u16,
    },
    InvalidBounds {
        minimum: u32,
        maximum: u32,
    },
    ConflictingCandidates {
        candidates: usize,
    },
    /// Number of candidates evaluated when none remained valid.
    Candidates {
        count: usize,
    },
    /// Location and category of a JSON contract violation; serde messages may echo input.
    JsonContract {
        category: String,
        line: usize,
        column: usize,
    },
    /// Fixed parser error vocabulary from the `url` crate.
    UrlParse {
        cause: String,
    },
}

impl NormalizationIssueDetails {
    pub fn json_contract(error: &serde_json::Error) -> Self {
        let category = match error.classify() {
            serde_json::error::Category::Io => "IO",
            serde_json::error::Category::Syntax => "SYNTAX",
            serde_json::error::Category::Data => "DATA",
            serde_json::error::Category::Eof => "EOF",
        };
        Self::JsonContract {
            category: category.to_owned(),
            line: error.line(),
            column: error.column(),
        }
    }

    pub fn url_parse(error: &url::ParseError) -> Self {
        Self::UrlParse {
            cause: error.to_string(),
        }
    }
}

/// One unresolved or failed normalization observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizationIssue {
    pub stage: NormalizationStage,
    pub field: NormalizationField,
    pub reason: NormalizationIssueReason,
    pub scope: NormalizationIssueScope,
    pub handling: NormalizationIssueHandling,
    pub details: NormalizationIssueDetails,
}

impl NormalizationIssue {
    pub const fn field(
        field: NormalizationField,
        reason: NormalizationIssueReason,
        handling: NormalizationIssueHandling,
        details: NormalizationIssueDetails,
    ) -> Self {
        Self {
            stage: NormalizationStage::Field,
            field,
            reason,
            scope: NormalizationIssueScope::CandidateData,
            handling,
            details,
        }
    }

    pub const fn envelope(
        field: NormalizationField,
        reason: NormalizationIssueReason,
        details: NormalizationIssueDetails,
    ) -> Self {
        Self {
            stage: NormalizationStage::Envelope,
            field,
            reason,
            scope: NormalizationIssueScope::CandidateData,
            handling: NormalizationIssueHandling::RejectedInput,
            details,
        }
    }

    pub const fn with_scope(mut self, scope: NormalizationIssueScope) -> Self {
        self.scope = scope;
        self
    }

    /// Typed field issue for a hard field-level [`NormalizationError`].
    pub fn from_normalization_error(error: &NormalizationError) -> Self {
        use NormalizationIssueReason as Reason;
        let rejected = NormalizationIssueHandling::RejectedInput;
        let none = NormalizationIssueDetails::None;
        let issue = match error {
            NormalizationError::SourceListingIdEmpty => Self::field(
                NormalizationField::SourceListingId,
                Reason::SourceListingIdEmpty,
                rejected,
                none,
            ),
            NormalizationError::SourceListingIdInvalid(_) => Self::field(
                NormalizationField::SourceListingId,
                Reason::SourceListingIdInvalid,
                rejected,
                none,
            ),
            NormalizationError::TitleEmpty => Self::field(
                NormalizationField::Title,
                Reason::TitleEmpty,
                rejected,
                none,
            ),
            NormalizationError::TitleUnknownLanguage { .. } => Self::field(
                NormalizationField::Title,
                Reason::TitleUnknownLanguage,
                rejected,
                none,
            ),
            NormalizationError::DescriptionUnknownLanguage { .. } => Self::field(
                NormalizationField::Description,
                Reason::DescriptionUnknownLanguage,
                rejected,
                none,
            ),
            NormalizationError::PriceUnknownCurrency { field } => Self::field(
                (*field).into(),
                Reason::PriceUnknownCurrency,
                rejected,
                none,
            ),
            NormalizationError::PriceParseError { field } => {
                Self::field((*field).into(), Reason::PriceParseFailed, rejected, none)
            }
            NormalizationError::PriceNegative { field } => {
                Self::field((*field).into(), Reason::PriceNegative, rejected, none)
            }
            NormalizationError::PriceExcessPrecision { field } => Self::field(
                (*field).into(),
                Reason::PriceExcessPrecision,
                rejected,
                none,
            ),
            NormalizationError::PriceMachineDecimalFallbackCurrencyRequired { field } => {
                Self::field(
                    (*field).into(),
                    Reason::MachineDecimalFallbackCurrencyRequired,
                    rejected,
                    none,
                )
            }
            NormalizationError::InvalidImageUrl(source) => Self::field(
                NormalizationField::Images,
                Reason::ImageUrlInvalid,
                rejected,
                NormalizationIssueDetails::url_parse(source),
            ),
            NormalizationError::NoValidImages { candidates } => Self::field(
                NormalizationField::Images,
                Reason::NoValidImages,
                rejected,
                NormalizationIssueDetails::Candidates { count: *candidates },
            ),
            NormalizationError::AvailabilityTextTooLong { len, max } => Self::field(
                NormalizationField::Availability,
                Reason::AvailabilityTextTooLong,
                rejected,
                NormalizationIssueDetails::SizeLimit {
                    observed: *len,
                    limit: *max,
                },
            ),
            NormalizationError::AvailabilityTextEmbeddedNul => Self::field(
                NormalizationField::Availability,
                Reason::AvailabilityEmbeddedNul,
                rejected,
                none,
            ),
            NormalizationError::AvailabilityRegexSetCompilationFailed => Self::field(
                NormalizationField::Availability,
                Reason::AvailabilityRulesUnavailable,
                rejected,
                none,
            ),
        };
        issue.with_scope(error.failure_scope().into())
    }

    /// Soft availability miss: the supplied label is preserved as `UNCHANGED`.
    pub const fn availability_unsupported(reason: AvailabilityUnsupportedReason) -> Self {
        let reason = match reason {
            AvailabilityUnsupportedReason::UnrecognizedLabel => {
                NormalizationIssueReason::AvailabilityUnrecognized
            }
            AvailabilityUnsupportedReason::ConflictingQuantity => {
                NormalizationIssueReason::AvailabilityConflictingQuantity
            }
            AvailabilityUnsupportedReason::QualifiedQuantity => {
                NormalizationIssueReason::AvailabilityQualifiedQuantity
            }
        };
        Self::field(
            NormalizationField::Availability,
            reason,
            NormalizationIssueHandling::PreservedField,
            NormalizationIssueDetails::None,
        )
    }

    /// Soft shipping miss: no shipping assertion is made for this observation.
    pub fn shipping_unsupported(reason: &ShippingUnsupportedReason) -> Self {
        use NormalizationIssueReason as Reason;
        let (reason, details) = match *reason {
            ShippingUnsupportedReason::TooManyFragments { observed, limit } => (
                Reason::ShippingTooManyFragments,
                NormalizationIssueDetails::SizeLimit { observed, limit },
            ),
            ShippingUnsupportedReason::TextTooLarge { observed, limit } => (
                Reason::ShippingTextTooLarge,
                NormalizationIssueDetails::SizeLimit { observed, limit },
            ),
            ShippingUnsupportedReason::UnsupportedCharacter => (
                Reason::ShippingUnsupportedCharacter,
                NormalizationIssueDetails::None,
            ),
            ShippingUnsupportedReason::ConflictingPrice => (
                Reason::ShippingConflictingPrice,
                NormalizationIssueDetails::ConflictingCandidates { candidates: 2 },
            ),
            ShippingUnsupportedReason::ConflictingHandling => (
                Reason::ShippingConflictingHandling,
                NormalizationIssueDetails::ConflictingCandidates { candidates: 2 },
            ),
            ShippingUnsupportedReason::InvalidHandlingRange { minimum, maximum } => (
                Reason::ShippingInvalidHandlingRange,
                NormalizationIssueDetails::InvalidBounds { minimum, maximum },
            ),
            ShippingUnsupportedReason::UnrecognizedFieldClause => (
                Reason::ShippingUnrecognizedFieldClause,
                NormalizationIssueDetails::None,
            ),
            ShippingUnsupportedReason::UnrecognizedDeliveryClause => (
                Reason::ShippingUnrecognizedDeliveryClause,
                NormalizationIssueDetails::None,
            ),
            ShippingUnsupportedReason::ConditionalClause => (
                Reason::ShippingConditionalClause,
                NormalizationIssueDetails::None,
            ),
            ShippingUnsupportedReason::ContradictoryAssertions => (
                Reason::ShippingContradictoryAssertions,
                NormalizationIssueDetails::None,
            ),
            ShippingUnsupportedReason::RulesUnavailable => (
                Reason::ShippingRulesUnavailable,
                NormalizationIssueDetails::None,
            ),
        };
        let issue = Self::field(
            NormalizationField::Shipping,
            reason,
            NormalizationIssueHandling::UnresolvedField,
            details,
        );
        if matches!(reason, Reason::ShippingRulesUnavailable) {
            issue.with_scope(NormalizationIssueScope::System)
        } else {
            issue
        }
    }

    /// Supplied display price whose currency could not be determined; preserved as `UNCHANGED`.
    pub fn price_unknown_currency(field: PriceField) -> Self {
        Self::field(
            field.into(),
            NormalizationIssueReason::PriceUnknownCurrency,
            NormalizationIssueHandling::PreservedField,
            NormalizationIssueDetails::None,
        )
    }
}

/// Typed price error for a field. Unknown currency is classified by the caller's policy.
pub fn price_normalization_error(
    error: PriceNormalizationError,
    field: PriceField,
) -> NormalizationError {
    match error {
        PriceNormalizationError::UnknownCurrency => {
            NormalizationError::PriceUnknownCurrency { field }
        }
        PriceNormalizationError::ParseFailure => NormalizationError::PriceParseError { field },
        PriceNormalizationError::NegativeAmount => NormalizationError::PriceNegative { field },
        PriceNormalizationError::ExcessPrecision => {
            NormalizationError::PriceExcessPrecision { field }
        }
    }
}

/// Typed availability error for the availability field.
pub const fn availability_normalization_error(
    error: AvailabilityNormalizationError,
) -> NormalizationError {
    match error {
        AvailabilityNormalizationError::InputTooLong { len, max } => {
            NormalizationError::AvailabilityTextTooLong { len, max }
        }
        AvailabilityNormalizationError::EmbeddedNul => {
            NormalizationError::AvailabilityTextEmbeddedNul
        }
        AvailabilityNormalizationError::RegexSetCompilationFailed => {
            NormalizationError::AvailabilityRegexSetCompilationFailed
        }
    }
}

/// All issues observed during one evaluation, in evaluation order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NormalizationReport {
    issues: Vec<NormalizationIssue>,
}

impl NormalizationReport {
    pub const fn new() -> Self {
        Self { issues: Vec::new() }
    }

    pub fn push(&mut self, issue: NormalizationIssue) {
        self.issues.push(issue);
    }

    pub fn issues(&self) -> &[NormalizationIssue] {
        &self.issues
    }

    pub fn into_issues(self) -> Vec<NormalizationIssue> {
        self.issues
    }

    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn has_system_issue(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.scope == NormalizationIssueScope::System)
    }
}

impl From<Vec<NormalizationIssue>> for NormalizationReport {
    fn from(issues: Vec<NormalizationIssue>) -> Self {
        Self { issues }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_match_their_persisted_representation() {
        use strum::IntoEnumIterator;
        for reason in NormalizationIssueReason::iter() {
            assert_eq!(
                serde_json::Value::String(reason.as_str().to_owned()),
                serde_json::to_value(reason).unwrap()
            );
        }
    }

    #[test]
    fn issues_serialize_with_stable_codes_and_without_input_text() {
        let issue = NormalizationIssue::from_normalization_error(
            &NormalizationError::TitleUnknownLanguage {
                text: "private text".to_owned(),
            },
        );
        let encoded = serde_json::to_string(&issue).unwrap();
        assert_eq!(
            r#"{"stage":"FIELD","field":"TITLE","reason":"TITLE_UNKNOWN_LANGUAGE","scope":"CANDIDATE_DATA","handling":"REJECTED_INPUT","details":{"kind":"NONE"}}"#,
            encoded
        );
    }
}
