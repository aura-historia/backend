use strum::IntoEnumIterator;

mod keywords;
mod nazi_germany;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, strum_macros::EnumIter)]
pub enum SensitiveContentCategory {
    NaziGermany,
}

impl SensitiveContentCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NaziGermany => "NAZI_GERMANY",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|category| category.as_str() == value)
    }

    /// The text heuristic of this category. Assessment checks categories in declaration order.
    fn keywords(self) -> &'static keywords::KeywordSet {
        match self {
            Self::NaziGermany => &nazi_germany::NAZI_GERMANY_KEYWORDS,
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ContentPolicyDecision {
    Allowed,
    RequiresConsent(SensitiveContentCategory),
}

impl ContentPolicyDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "ALLOWED",
            Self::RequiresConsent(_) => "REQUIRES_CONSENT",
        }
    }

    /// Decodes a decision code and its category code. Only `ALLOWED` without a category and
    /// `REQUIRES_CONSENT` with a known category are valid.
    pub fn from_codes(decision: &str, category: Option<&str>) -> Option<Self> {
        match (decision, category) {
            ("ALLOWED", None) => Some(Self::Allowed),
            ("REQUIRES_CONSENT", Some(category)) => {
                SensitiveContentCategory::from_code(category).map(Self::RequiresConsent)
            }
            _ => None,
        }
    }
}

pub fn may_show_product_listing_images(
    decision: Option<ContentPolicyDecision>,
    show_unassessed_or_sensitive_content: bool,
) -> bool {
    show_unassessed_or_sensitive_content || matches!(decision, Some(ContentPolicyDecision::Allowed))
}

/// Assesses source listing text. Blank or absent text is not an assessment.
pub fn assess_listing_text(
    title: Option<&str>,
    description: Option<&str>,
) -> Option<ContentPolicyDecision> {
    let parts = [title, description]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();

    if parts.is_empty() {
        return None;
    }

    let text = parts.join(" ").to_lowercase();
    Some(
        SensitiveContentCategory::iter()
            .find(|category| category.keywords().matches(&text))
            .map_or(
                ContentPolicyDecision::Allowed,
                ContentPolicyDecision::RequiresConsent,
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_keep_canonical_content_policy_identifiers_exact() {
        assert_eq!(ContentPolicyDecision::Allowed.as_str(), "ALLOWED");
        assert_eq!(
            ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany).as_str(),
            "REQUIRES_CONSENT"
        );
        assert_eq!(
            SensitiveContentCategory::NaziGermany.as_str(),
            "NAZI_GERMANY"
        );

        assert_eq!(
            SensitiveContentCategory::from_code("NAZI_GERMANY"),
            Some(SensitiveContentCategory::NaziGermany)
        );
        assert_eq!(SensitiveContentCategory::from_code("nazi_germany"), None);
        assert_eq!(
            SensitiveContentCategory::iter()
                .map(SensitiveContentCategory::as_str)
                .collect::<HashSet<_>>()
                .len(),
            SensitiveContentCategory::iter().count(),
        );
    }

    #[rstest::rstest]
    #[case("ALLOWED", None, Some(ContentPolicyDecision::Allowed))]
    #[case(
        "REQUIRES_CONSENT",
        Some("NAZI_GERMANY"),
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany))
    )]
    #[case("ALLOWED", Some("NAZI_GERMANY"), None)]
    #[case("REQUIRES_CONSENT", None, None)]
    #[case("REQUIRES_CONSENT", Some("UNKNOWN"), None)]
    #[case("allowed", None, None)]
    fn should_decode_only_valid_decision_and_category_codes(
        #[case] decision: &str,
        #[case] category: Option<&str>,
        #[case] expected: Option<ContentPolicyDecision>,
    ) {
        assert_eq!(
            ContentPolicyDecision::from_codes(decision, category),
            expected
        );
    }

    #[rstest::rstest]
    #[case(None, false, false)]
    #[case(None, true, true)]
    #[case(Some(ContentPolicyDecision::Allowed), false, true)]
    #[case(Some(ContentPolicyDecision::Allowed), true, true)]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        false,
        false
    )]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        true,
        true
    )]
    fn should_apply_content_visibility_policy(
        #[case] decision: Option<ContentPolicyDecision>,
        #[case] preference: bool,
        #[case] visible: bool,
    ) {
        assert_eq!(
            may_show_product_listing_images(decision, preference),
            visible
        );
    }

    #[test]
    fn should_return_no_assessment_when_title_is_empty() {
        assert_eq!(assess_listing_text(Some(""), None), None);
    }
}
