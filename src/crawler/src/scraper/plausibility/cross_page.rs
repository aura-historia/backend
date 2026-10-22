//! Cross-page plausibility checks: one candidate's extractions of the review pages compared
//! with each other. Each review page shows a different product, so a `title`, an identity
//! or an image that two pages share names the shop, the category or a default share image
//! rather than the product. Runtime extraction sees one page at a time and cannot run them.

use super::PlausibilityField;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::css_selector::rule::{has_known_image_extension, split_image_candidate_group};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use url::Url;

/// The check a value failed against the candidate's extractions of the other pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CrossPageCheck {
    /// The same value is extracted from the page of another product.
    RepeatedAcrossPages,
}

impl CrossPageCheck {
    const fn description(self) -> &'static str {
        match self {
            Self::RepeatedAcrossPages => "repeats a value of another product's page",
        }
    }
}

/// One field of a page whose values another page of the same candidate repeats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossPageFinding {
    pub field: PlausibilityField,
    pub check: CrossPageCheck,
    /// This page's values that another page shares, as extracted from this page.
    pub values: Vec<String>,
}

impl fmt::Display for CrossPageFinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "`{}` {}",
            self.field.as_str(),
            self.check.description()
        )
    }
}

/// The cross-page checks one page failed, in schema field order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CrossPageFindings(Vec<CrossPageFinding>);

/// One candidate's extraction of one review page.
pub struct PageExtraction<'a> {
    /// The page URL, against which relative image URLs resolve.
    pub url: &'a str,
    pub raw: &'a RawExtractedProduct,
}

impl CrossPageFindings {
    /// Compares the extractions of `pages`, each of a different product, and returns the
    /// findings of each page in the order of `pages`. Blank values are left to the
    /// per-page checks.
    pub fn of(pages: &[PageExtraction<'_>]) -> Vec<Self> {
        let identities = repeated_values(pages, |page| {
            non_blank(page.raw.source_listing_id.trim())
                .map(|identity| (identity.to_owned(), identity.to_owned()))
                .into_iter()
                .collect()
        });
        let titles = repeated_values(pages, |page| {
            non_blank(page.raw.title.trim())
                .map(|title| (collapse_whitespace(title), title.to_owned()))
                .into_iter()
                .collect()
        });
        let images = repeated_values(pages, image_identities);

        identities
            .into_iter()
            .zip(titles)
            .zip(images)
            .map(|((identities, titles), images)| {
                Self(
                    [
                        (PlausibilityField::SourceListingId, identities),
                        (PlausibilityField::Title, titles),
                        (PlausibilityField::Images, images),
                    ]
                    .into_iter()
                    .filter(|(_, values)| !values.is_empty())
                    .map(|(field, values)| CrossPageFinding {
                        field,
                        check: CrossPageCheck::RepeatedAcrossPages,
                        values,
                    })
                    .collect(),
                )
            })
            .collect()
    }

    pub fn as_slice(&self) -> &[CrossPageFinding] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// For each page, its values whose comparison key also occurs on another page. `keyed`
/// yields a page's `(key, value)` pairs.
fn repeated_values(
    pages: &[PageExtraction<'_>],
    keyed: impl Fn(&PageExtraction<'_>) -> Vec<(String, String)>,
) -> Vec<Vec<String>> {
    let keyed_pages: Vec<Vec<(String, String)>> = pages.iter().map(keyed).collect();
    let mut pages_per_key: HashMap<&str, usize> = HashMap::new();
    for page in &keyed_pages {
        let keys: HashSet<&str> = page.iter().map(|(key, _)| key.as_str()).collect();
        for key in keys {
            *pages_per_key.entry(key).or_default() += 1;
        }
    }
    keyed_pages
        .iter()
        .map(|page| {
            let mut reported = HashSet::new();
            page.iter()
                .filter(|(key, _)| pages_per_key[key.as_str()] > 1 && reported.insert(key))
                .map(|(_, value)| value.clone())
                .collect()
        })
        .collect()
}

/// The primary candidate of each image candidate group, keyed by the image it names.
fn image_identities(page: &PageExtraction<'_>) -> Vec<(String, String)> {
    let base_url = Url::parse(page.url).ok();
    page.raw
        .images
        .iter()
        .filter_map(|group| split_image_candidate_group(group).into_iter().next())
        .filter_map(|candidate| non_blank(candidate.trim()))
        .map(|candidate| {
            (
                image_identity(base_url.as_ref(), candidate),
                candidate.to_owned(),
            )
        })
        .collect()
}

/// The image `candidate` names, resolved against the page URL. A fragment never names
/// another image; neither does the query of a path that names an image file, which
/// carries a size, a cache buster or a per-page CDN signature (In A Nutshell signs its
/// default share image differently on every page).
fn image_identity(base_url: Option<&Url>, candidate: &str) -> String {
    let resolved = match base_url {
        Some(base_url) => base_url.join(candidate),
        None => Url::parse(candidate),
    };
    let Ok(mut url) = resolved else {
        return candidate.to_owned();
    };
    url.set_fragment(None);
    if has_known_image_extension(url.path()) {
        url.set_query(None);
    }
    url.into()
}

fn non_blank(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::IMAGE_CANDIDATE_SEPARATOR;

    fn product(identity: &str, title: &str, images: &[&str]) -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: identity.to_owned(),
            title: title.to_owned(),
            description: Vec::new(),
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "Available".to_owned(),
            images: images.iter().map(|image| (*image).to_owned()).collect(),
            raw_attributes: Default::default(),
        }
    }

    fn findings_of(pages: &[(&str, RawExtractedProduct)]) -> Vec<Vec<CrossPageFinding>> {
        let pages: Vec<PageExtraction<'_>> = pages
            .iter()
            .map(|(url, raw)| PageExtraction { url, raw })
            .collect();
        CrossPageFindings::of(&pages)
            .into_iter()
            .map(|findings| findings.as_slice().to_vec())
            .collect()
    }

    fn repeated(field: PlausibilityField, values: &[&str]) -> CrossPageFinding {
        CrossPageFinding {
            field,
            check: CrossPageCheck::RepeatedAcrossPages,
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    mod distinct_products {
        use super::*;

        #[test]
        fn should_report_nothing_when_identity_title_and_images_differ_per_page() {
            let findings = findings_of(&[
                (
                    "https://shop.example/chair",
                    product("13803", "Walnut chair", &["/chair-1.jpg", "/chair-2.jpg"]),
                ),
                (
                    "https://shop.example/table",
                    product("13892", "Oak table", &["/table-1.jpg"]),
                ),
            ]);

            assert_eq!(findings, vec![vec![], vec![]]);
        }

        #[test]
        fn should_report_nothing_for_a_single_page() {
            let findings = findings_of(&[(
                "https://shop.example/chair",
                product("top", "Antiques - Navarro", &["/share.jpg", "/share.jpg"]),
            )]);

            assert_eq!(findings, vec![vec![]]);
        }

        #[test]
        fn should_leave_blank_identities_and_titles_to_the_per_page_checks() {
            let findings = findings_of(&[
                ("https://shop.example/a", product(" ", "\n", &[])),
                ("https://shop.example/b", product("", " ", &[])),
            ]);

            assert_eq!(findings, vec![vec![], vec![]]);
        }

        #[test]
        fn should_keep_query_strings_that_select_the_image() {
            let findings = findings_of(&[
                (
                    "https://shop.example/a",
                    product("1", "Chair", &["/image.php?id=1"]),
                ),
                (
                    "https://shop.example/b",
                    product("2", "Table", &["/image.php?id=2"]),
                ),
            ]);

            assert_eq!(findings, vec![vec![], vec![]]);
        }

        #[test]
        fn should_compare_only_the_primary_candidate_of_an_image_group() {
            let findings = findings_of(&[
                (
                    "https://shop.example/a",
                    product(
                        "1",
                        "Chair",
                        &[&format!(
                            "/chair.jpg{IMAGE_CANDIDATE_SEPARATOR}/placeholder.gif"
                        )],
                    ),
                ),
                (
                    "https://shop.example/b",
                    product(
                        "2",
                        "Table",
                        &[&format!(
                            "/table.jpg{IMAGE_CANDIDATE_SEPARATOR}/placeholder.gif"
                        )],
                    ),
                ),
            ]);

            assert_eq!(findings, vec![vec![], vec![]]);
        }
    }

    mod repeated_values {
        use super::*;
        use PlausibilityField::{Images, SourceListingId, Title};

        /// Navarro's untitled sold items share the document `<title>`, and Vauclair's
        /// `<body id>` is the same on every product page.
        #[test]
        fn should_report_an_identity_and_a_title_that_another_page_repeats() {
            let findings = findings_of(&[
                (
                    "https://shop.example/sold/item-422460",
                    product("top", "Sold items - Navarro", &[]),
                ),
                (
                    "https://shop.example/sold/item-458585",
                    product("top", "Sold items  -\nNavarro", &[]),
                ),
                (
                    "https://shop.example/tables/console-7188848",
                    product("top", "Decorative console - Tables - Navarro", &[]),
                ),
            ]);

            assert_eq!(
                findings,
                vec![
                    vec![
                        repeated(SourceListingId, &["top"]),
                        repeated(Title, &["Sold items - Navarro"]),
                    ],
                    vec![
                        repeated(SourceListingId, &["top"]),
                        repeated(Title, &["Sold items  -\nNavarro"]),
                    ],
                    vec![repeated(SourceListingId, &["top"])],
                ]
            );
        }

        /// Marhamchurch's photo-less products carry the site's default `og:image`.
        #[test]
        fn should_report_an_image_that_another_product_page_shows() {
            let share_image = "https://www.shop.example/uploads/flemish-tapestry.jpg";
            let findings = findings_of(&[
                (
                    "https://www.shop.example/antique/ewer/",
                    product("26650", "Ewer", &[share_image]),
                ),
                (
                    "https://www.shop.example/antique/saint/",
                    product("26357", "Saint", &["/uploads/flemish-tapestry.jpg#zoom"]),
                ),
                (
                    "https://www.shop.example/antique/chest/",
                    product("24821", "Chest", &["/uploads/chest.jpg"]),
                ),
            ]);

            assert_eq!(
                findings,
                vec![
                    vec![repeated(Images, &[share_image])],
                    vec![repeated(Images, &["/uploads/flemish-tapestry.jpg#zoom"])],
                    vec![],
                ]
            );
        }

        /// In A Nutshell signs its default share image with a different query per page.
        #[test]
        fn should_report_a_repeated_image_whose_signature_differs_per_page() {
            let lamp = |expires: &str| {
                format!(
                    "https://cdn.example/opt/lamp-illustration-1920w.png?Expires={expires}&Signature=x{expires}"
                )
            };
            let first_lamp = lamp("1795219033");
            let second_lamp = lamp("1795213981");
            let findings = findings_of(&[
                (
                    "https://shop.example/stock/lights-p807376735",
                    product(
                        "p807376735",
                        "Pendant lights",
                        &["/lights.jpg", &first_lamp],
                    ),
                ),
                (
                    "https://shop.example/stock/cabinet-p857614616",
                    product(
                        "p857614616",
                        "Pine cabinet",
                        &["/cabinet.jpg", &second_lamp],
                    ),
                ),
            ]);

            assert_eq!(
                findings,
                vec![
                    vec![repeated(Images, &[&first_lamp])],
                    vec![repeated(Images, &[&second_lamp])],
                ]
            );
        }

        #[test]
        fn should_report_each_repeated_image_once_per_page() {
            let findings = findings_of(&[
                (
                    "https://shop.example/a",
                    product("1", "Chair", &["/logo.png", "/logo.png", "/badge.png"]),
                ),
                (
                    "https://shop.example/b",
                    product("2", "Table", &["/badge.png", "/logo.png"]),
                ),
            ]);

            assert_eq!(
                findings,
                vec![
                    vec![repeated(Images, &["/logo.png", "/badge.png"])],
                    vec![repeated(Images, &["/badge.png", "/logo.png"])],
                ]
            );
        }
    }

    mod serialization {
        use super::*;

        #[test]
        fn should_serialize_with_schema_field_names_and_a_stable_check_code() {
            let findings = findings_of(&[
                ("https://shop.example/a", product("1", "Same", &[])),
                ("https://shop.example/b", product("2", "Same", &[])),
            ]);

            assert_eq!(
                serde_json::to_value(&findings[0])
                    .unwrap_or_else(|error| panic!("findings must serialize: {error}")),
                serde_json::json!([{
                    "field": "title",
                    "check": "REPEATED_ACROSS_PAGES",
                    "values": ["Same"]
                }])
            );
        }
    }
}
