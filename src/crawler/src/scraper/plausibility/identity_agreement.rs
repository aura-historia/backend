//! Identity agreement on one review page: the identity each schema derives from the page
//! compared with the other schemas' and with the identity the page's listing was captured
//! under. Runtime selection picks one schema per page, and a scrape whose identity differs
//! from the captured one is rejected as a conflict, so schemas that disagree on a page
//! split one product into two listings or stop its updates, as when a sold page loses the
//! element the other schemas read the identity from.

use super::PlausibilityField;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use product_listing_core::source_listing_id::SourceListingId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// The identity a page's extraction disagrees with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IdentityAgreementCheck {
    /// Another schema that applies to the page derives a different identity from it.
    DiffersFromOtherSchemas,
    /// The page's listing was captured under a different identity.
    DiffersFromCapturedIdentity,
}

impl IdentityAgreementCheck {
    /// Every check, so tests can pin how the review console names each code.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 2] = [
        Self::DiffersFromOtherSchemas,
        Self::DiffersFromCapturedIdentity,
    ];

    /// How the review console describes the check.
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::DiffersFromOtherSchemas => {
                "differs from the identity another schema derives from this page"
            }
            Self::DiffersFromCapturedIdentity => {
                "differs from the identity the listing was captured under"
            }
        }
    }
}

/// One identity of a page's extraction that another identity of the page contradicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityAgreementFinding {
    /// Always `source_listing_id`; named like the other findings so the review console
    /// shows it next to the field.
    pub field: PlausibilityField,
    pub check: IdentityAgreementCheck,
    /// The contradicting identities: the other schemas' or the captured one.
    pub values: Vec<String>,
}

impl fmt::Display for IdentityAgreementFinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "`{}` {}",
            self.field.as_str(),
            self.check.description()
        )
    }
}

/// The identity agreement checks one candidate's extraction of one page failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityAgreementFindings(Vec<IdentityAgreementFinding>);

impl IdentityAgreementFindings {
    /// Compares the identities the candidates derive from one page, one extraction per
    /// candidate and `None` where runtime selection would not use the candidate for the page,
    /// with each other and with `captured_identity`. Returns the findings of each candidate
    /// in the order of `extractions`. Blank and invalid identities are left to the per-page
    /// checks.
    pub fn of(
        extractions: &[Option<&RawExtractedProduct>],
        captured_identity: Option<&str>,
    ) -> Vec<Self> {
        let identities: Vec<Option<String>> = extractions
            .iter()
            .map(|raw| raw.and_then(|raw| capturable_identity(&raw.source_listing_id)))
            .collect();
        identities
            .iter()
            .map(|identity| {
                let Some(identity) = identity else {
                    return Self::default();
                };
                let other_schemas: BTreeSet<&String> = identities
                    .iter()
                    .flatten()
                    .filter(|other| *other != identity)
                    .collect();
                let mut findings = Vec::new();
                if !other_schemas.is_empty() {
                    findings.push(IdentityAgreementFinding {
                        field: PlausibilityField::SourceListingId,
                        check: IdentityAgreementCheck::DiffersFromOtherSchemas,
                        values: other_schemas.into_iter().cloned().collect(),
                    });
                }
                if let Some(captured) = captured_identity.filter(|captured| captured != identity) {
                    findings.push(IdentityAgreementFinding {
                        field: PlausibilityField::SourceListingId,
                        check: IdentityAgreementCheck::DiffersFromCapturedIdentity,
                        values: vec![captured.to_owned()],
                    });
                }
                Self(findings)
            })
            .collect()
    }

    pub fn as_slice(&self) -> &[IdentityAgreementFinding] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The identity a raw extraction is captured under when it yields one, as the scraper
/// compares it with the captured identity.
fn capturable_identity(raw: &str) -> Option<String> {
    SourceListingId::try_from(raw).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(identity: &str) -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: identity.to_owned(),
            title: "Georgian Mahogany Chair".to_owned(),
            description: Vec::new(),
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "Sold".to_owned(),
            images: Vec::new(),
            raw_attributes: Default::default(),
        }
    }

    fn finding(check: IdentityAgreementCheck, values: &[&str]) -> IdentityAgreementFinding {
        IdentityAgreementFinding {
            field: PlausibilityField::SourceListingId,
            check,
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    #[test]
    fn should_report_each_schema_the_identities_of_the_others_when_they_disagree_on_a_page() {
        let button = product("53036");
        let form = product("53036");
        let container = product("product-53036");

        let findings =
            IdentityAgreementFindings::of(&[Some(&button), Some(&form), Some(&container)], None);

        assert_eq!(
            findings[0].as_slice(),
            [finding(
                IdentityAgreementCheck::DiffersFromOtherSchemas,
                &["product-53036"]
            )]
        );
        assert_eq!(findings[1], findings[0]);
        assert_eq!(
            findings[2].as_slice(),
            [finding(
                IdentityAgreementCheck::DiffersFromOtherSchemas,
                &["53036"]
            )]
        );
    }

    #[test]
    fn should_report_an_identity_that_differs_from_the_captured_one_when_it_is_the_only_schema_on_the_page()
     {
        let container = product("product-53036");

        let findings = IdentityAgreementFindings::of(&[None, Some(&container)], Some("53036"));

        assert!(findings[0].is_empty());
        assert_eq!(
            findings[1].as_slice(),
            [finding(
                IdentityAgreementCheck::DiffersFromCapturedIdentity,
                &["53036"]
            )]
        );
    }

    #[test]
    fn should_not_report_schemas_that_derive_the_captured_identity() {
        let button = product("53036");
        let form = product(" 53036 ");

        let findings = IdentityAgreementFindings::of(&[Some(&button), Some(&form)], Some("53036"));

        assert!(findings.iter().all(IdentityAgreementFindings::is_empty));
    }

    #[test]
    fn should_leave_blank_and_withdrawn_identities_to_the_per_page_checks() {
        let blank = product("  ");
        let identified = product("53036");

        let findings =
            IdentityAgreementFindings::of(&[Some(&blank), None, Some(&identified)], None);

        assert!(findings.iter().all(IdentityAgreementFindings::is_empty));
    }
}
