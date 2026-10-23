//! Public summary of an assessed shipping, collection or option outcome.
//! A deliberate projection of the internal reason trace with a bounded, documented vocabulary.
use schemars::JsonSchema;
use serde::Serialize;
use shipping_core::{
    assessment::{
        AssessmentReason, CollectionAssessment, MissingInput, PredicateResult, ServiceAssessment,
        ShippingAssessment,
    },
    pricing::QualifyingScope,
    terms::ShippingEligibility,
};
use std::collections::BTreeSet;

/// Assessed eligibility, or the inputs and explanations leaving it undetermined. Omitted
/// eligibility means unknown or unasserted; only an explicit `NOT_OFFERED` is a negative.
#[derive(Debug, Default, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "DeliveryAssessmentSummaryData")]
pub struct AssessmentSummaryData {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "super::values::eligibility")]
    eligibility: Option<&'static str>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    #[schemars(length(min = 1))]
    required_inputs: BTreeSet<RequiredInputData>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    #[schemars(length(min = 1))]
    explanations: BTreeSet<ExplanationData>,
}

/// Inputs a caller can supply whose absence leaves eligibility undetermined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "DeliveryAssessmentRequiredInput")]
pub enum RequiredInputData {
    Destination,
    DestinationCountry,
    DestinationSubdivision,
    PostalCode,
    Quantity,
    Lots,
    Shipments,
    SellerOrders,
    Variant,
}

/// Why eligibility remains undetermined beyond missing caller inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(tag = "code", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "DeliveryAssessmentExplanation")]
pub enum ExplanationData {
    /// Partial coverage neither includes nor excludes the destination or service.
    IncompleteCoverage,
    /// A condition depends on a qualifying amount at this scope, which browsing never supplies.
    QualifyingAmountRequired {
        #[schemars(schema_with = "super::values::qualifying_scope")]
        scope: &'static str,
    },
    /// Advertised terms or their seller-side facts cannot be interpreted conservatively.
    UnresolvedTerms,
    /// Collection depends on shipping eligibility, which remains undetermined.
    ShippingUndetermined,
    /// The applicable advertised terms assert no eligibility.
    NotAsserted,
}

enum Meaning {
    Input(RequiredInputData),
    Explanation(ExplanationData),
    /// Unasserted terms, internal pointers, negatives and price facts shown elsewhere.
    Omitted,
}

fn meaning(reason: &AssessmentReason) -> Meaning {
    use AssessmentReason::*;
    use Meaning::{Explanation, Omitted};
    match reason {
        MissingInput(input) => missing_input(*input),
        IncompleteCoverage => Explanation(ExplanationData::IncompleteCoverage),
        ShippingDependencyUndetermined => Explanation(ExplanationData::ShippingUndetermined),
        NoEligibilityAssertion => Explanation(ExplanationData::NotAsserted),
        SourceMismatch
        | AmbiguousDependency
        | LocationAbsent(_)
        | LocationRetired(_)
        | ReferenceReleaseMismatch
        | UnresolvedArea
        | UnsupportedPostalShape
        | IncompleteClassMembership
        | VariantUnavailable
        | MixedVariantApplicability
        | CurrencyMismatch => Explanation(ExplanationData::UnresolvedTerms),
        NoSourceDefault
        | ProfileAbsent(_)
        | ProfileRetired(_)
        | SuppressedInheritance
        | AssertionCleared
        | ShippingDependencyReplaced
        | NoShippingAssertion
        | NoCollectionAssertion
        | EarlierZoneUndetermined { .. }
        | CountryMismatch
        | SubdivisionMismatch
        | PostalMismatch
        | ClassMismatch
        | VariantMismatch
        | QuantityOutsideRange
        | QualifyingThresholdNotMet
        | Excluded
        | OutsideCompleteCoverage
        | NoApplicableService
        | ShippingProhibited
        | UnknownChargeBasis
        | NoCurrencyAssertion
        | PriceOnRequest
        | ExternallyCalculatedPrice
        | ArithmeticOverflow => Omitted,
    }
}

fn missing_input(input: MissingInput) -> Meaning {
    use Meaning::*;
    use RequiredInputData::*;
    Input(match input {
        MissingInput::Destination => Destination,
        MissingInput::DestinationCountry => DestinationCountry,
        MissingInput::DestinationSubdivision => DestinationSubdivision,
        MissingInput::PostalCode => PostalCode,
        MissingInput::Quantity => Quantity,
        MissingInput::Lots => Lots,
        MissingInput::Shipments => Shipments,
        MissingInput::SellerOrders => SellerOrders,
        MissingInput::Variant => Variant,
        MissingInput::QualifyingAmount(scope) => {
            return Explanation(ExplanationData::QualifyingAmountRequired {
                scope: QualifyingScope::as_str(scope),
            });
        }
        MissingInput::OriginCountry | MissingInput::ClassMembership => {
            return Explanation(ExplanationData::UnresolvedTerms);
        }
    })
}

impl AssessmentSummaryData {
    /// Reasons explain only an undetermined outcome. With an outcome they can describe
    /// other services or charges, never what the outcome still depends on.
    fn new<'a>(
        eligibility: Option<ShippingEligibility>,
        undetermined_by: impl IntoIterator<Item = &'a AssessmentReason>,
    ) -> Self {
        let mut summary = Self {
            eligibility: eligibility.map(ShippingEligibility::as_str),
            ..Self::default()
        };
        if eligibility.is_none() {
            for reason in undetermined_by {
                match meaning(reason) {
                    Meaning::Input(input) => {
                        summary.required_inputs.insert(input);
                    }
                    Meaning::Explanation(explanation) => {
                        summary.explanations.insert(explanation);
                    }
                    Meaning::Omitted => {}
                }
            }
        }
        summary
    }
    pub(super) fn shipping(assessment: &ShippingAssessment) -> Self {
        Self::new(assessment.eligibility, &assessment.reasons)
    }
    /// `None` when collection is unasserted and nothing explains it.
    pub(super) fn collection(assessment: &CollectionAssessment) -> Option<Self> {
        Some(Self::new(assessment.eligibility, &assessment.reasons)).filter(|c| !c.is_empty())
    }
    /// Only applicability explains an option; charge reasons concern its price.
    pub(super) fn option(service: &ServiceAssessment) -> Self {
        Self::new(service.eligibility, service.applicability.reasons())
    }
    pub(super) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Options whose conditions definitely do not apply are not assessed alternatives.
pub(super) fn applicable(service: &ServiceAssessment) -> bool {
    !matches!(service.applicability, PredicateResult::NoMatch(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shipping_core::assessment::MissingInput;

    #[test]
    fn should_explain_only_undetermined_outcomes_with_deduplicated_public_codes() {
        let reasons = [
            AssessmentReason::MissingInput(MissingInput::Destination),
            AssessmentReason::EarlierZoneUndetermined { index: 3 },
            AssessmentReason::MissingInput(MissingInput::QualifyingAmount(
                QualifyingScope::SellerOrder,
            )),
            AssessmentReason::MissingInput(MissingInput::QualifyingAmount(QualifyingScope::Item)),
            AssessmentReason::MissingInput(MissingInput::ClassMembership),
            AssessmentReason::UnresolvedArea,
            AssessmentReason::ProfileAbsent(shipping_core::profile::ShippingProfileId::new()),
            AssessmentReason::MissingInput(MissingInput::Destination),
        ];
        assert_eq!(
            serde_json::json!({
                "requiredInputs": ["DESTINATION"],
                "explanations": [
                    {"code": "QUALIFYING_AMOUNT_REQUIRED", "scope": "ITEM"},
                    {"code": "QUALIFYING_AMOUNT_REQUIRED", "scope": "SELLER_ORDER"},
                    {"code": "UNRESOLVED_TERMS"}
                ]
            }),
            serde_json::to_value(AssessmentSummaryData::new(None, &reasons)).unwrap()
        );
        assert_eq!(
            serde_json::json!({"eligibility": "OFFERED"}),
            serde_json::to_value(AssessmentSummaryData::new(
                Some(ShippingEligibility::Offered),
                &reasons
            ))
            .unwrap()
        );
        assert!(
            AssessmentSummaryData::new(None, &[AssessmentReason::NoShippingAssertion]).is_empty()
        );
    }
}
