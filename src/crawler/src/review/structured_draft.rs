//! A deterministic first draft of a product schema from a review's pages: per field the first rule
//! of [`candidates`] that fits every page, judged against what the page visibly shows. Structured
//! data is used only where it holds machine values and only while the visible page agrees with it:
//! a JSON-LD price only when the page shows its amount, a JSON-LD availability only when no
//! visible label contradicts it; description and images always come from visible elements, and
//! the JSON-LD description and image list are only a lower bound they must reach. A field no rule
//! fits is left to the author agent, never guessed. The draft decides no approval: it is staged
//! for the approval guard and the reviewer agent like any candidate
//! (`docs/crawler/extraction-and-review.md`, structured-data draft).

mod candidates;

use crate::review::page_content_checks::StateReading;
use crate::review::page_signals::{ShownContent, shows_amount};
use crate::review::product_markup::{JsonLdProductReference, json_ld_product_reference};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::rule::{
    ExtractionKind, ExtractionRule, split_image_candidate_group,
};
use crate::scraper::plausibility::is_price_text;
use product_listing_core::listing_availability::ListingAvailability;
use scraper::Html;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// The fewest words and characters a drafted description holds on a page, so a label in a
/// `description` class is no description.
const MIN_DESCRIPTION_WORDS: usize = 8;
const MIN_DESCRIPTION_CHARS: usize = 50;

/// How much of the JSON-LD description, from its start, a visible description must contain: enough
/// to tell the product's text from another block, short enough to survive a shortened JSON-LD.
const DESCRIPTION_REFERENCE_CHARS: usize = 60;

/// A field the pages need that no rule of the draft fits, which the author agent writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum UndraftedField {
    State,
    Price,
    Description,
    Images,
}

/// The draft of a review's pages and the fields it could not draft.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StructuredDraft {
    pub(crate) schema: ProductCssSelectorSchema,
    pub(crate) undrafted: Vec<UndraftedField>,
}

impl StructuredDraft {
    /// Whether every field the pages need has a rule, so the draft can go to the reviewer without
    /// an author.
    pub(crate) fn is_complete(&self) -> bool {
        self.undrafted.is_empty()
    }
}

/// One review page as the draft reads it.
struct DraftPage {
    html: Html,
    shown: ShownContent,
    reference: JsonLdProductReference,
}

impl DraftPage {
    fn parse(raw_html: &str) -> Self {
        let html = Html::parse_document(raw_html);
        Self {
            shown: ShownContent::of(&html),
            reference: json_ld_product_reference(&html),
            html,
        }
    }

    fn values(&self, rule: &ExtractionRule) -> Option<Vec<String>> {
        let values = match rule.extract {
            ExtractionKind::ImageUrl => rule
                .apply_image_url_candidate_groups(&self.html)
                .ok()
                .map(|extraction| extraction.values),
            _ => rule.apply(&self.html).ok(),
        }?;
        let values: Vec<String> = values
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect();
        (!values.is_empty()).then_some(values)
    }

    fn identity(&self, rule: &ExtractionRule) -> Option<String> {
        rule.apply_as_identity(&self.html)
            .ok()?
            .into_iter()
            .map(|value| value.trim().to_owned())
            .find(|value| !value.is_empty())
    }
}

/// The draft of the review pages `pages` (their raw HTML), or `None` when no title rule anchors a
/// schema on every page: without one the draft would apply to no page.
pub(crate) fn draft_schema<'a>(
    pages: impl IntoIterator<Item = &'a str>,
) -> Option<StructuredDraft> {
    let pages: Vec<DraftPage> = pages.into_iter().map(DraftPage::parse).collect();
    if pages.is_empty() {
        return None;
    }
    let title = draft_title(&pages)?;
    let price = draft_price(&pages);
    let state = draft_state(&pages);
    let description = draft_description(&pages);
    let images = draft_images(&pages);
    let mut undrafted = Vec::new();
    if state.is_none() {
        undrafted.push(UndraftedField::State);
    }
    if price.is_none() && pages.iter().any(|page| page.shown.price_text) {
        undrafted.push(UndraftedField::Price);
    }
    if description.is_none() && pages.iter().any(|page| page.shown.description_block) {
        undrafted.push(UndraftedField::Description);
    }
    if images.is_none() {
        undrafted.push(UndraftedField::Images);
    }
    Some(StructuredDraft {
        schema: ProductCssSelectorSchema {
            source_listing_id: draft_source_listing_id(&pages),
            title: Some(title),
            description,
            price,
            price_estimate_min: None,
            price_estimate_max: None,
            state,
            images,
            raw_attributes: BTreeMap::new(),
        },
        undrafted,
    })
}

/// Whether `state` reads a sold or unavailable product on the page `raw_html`, as the approval
/// guard's sold-page check counts it.
pub(crate) fn shows_sold(state: &ExtractionRule, raw_html: &str) -> bool {
    DraftPage::parse(raw_html)
        .values(state)
        .and_then(|values| values.into_iter().next())
        .is_some_and(|value| StateReading::of(&value).shows_sold_or_unavailable())
}

/// The first of `rules` whose values `fits` accepts on every page.
fn first_fitting(
    pages: &[DraftPage],
    rules: Vec<ExtractionRule>,
    fits: impl Fn(&DraftPage, Option<&[String]>) -> bool,
) -> Option<ExtractionRule> {
    rules.into_iter().find(|rule| {
        let mut any_value = false;
        let every_page_fits = pages.iter().all(|page| {
            let values = page.values(rule);
            any_value |= values.is_some();
            fits(page, values.as_deref())
        });
        every_page_fits && any_value
    })
}

/// A title on every page, and not the same one on all of them, which would name the shop.
fn draft_title(pages: &[DraftPage]) -> Option<ExtractionRule> {
    candidates::title().into_iter().find(|rule| {
        let titles: Option<Vec<String>> = pages
            .iter()
            .map(|page| {
                page.values(rule)
                    .and_then(|values| values.into_iter().next())
            })
            .collect();
        titles.is_some_and(|titles| distinct_across_pages(&titles))
    })
}

/// The JSON-LD price where every page that states one shows its amount; else a visible price
/// element whose value is a price and, where the JSON-LD states one, its amount. A page without a
/// price fits only when it shows none.
fn draft_price(pages: &[DraftPage]) -> Option<ExtractionRule> {
    let json_ld = candidates::json_ld_price();
    let json_ld_prices: Vec<Option<String>> = pages
        .iter()
        .map(|page| {
            page.values(&json_ld)
                .and_then(|values| values.into_iter().next())
        })
        .collect();
    let mut rules = vec![json_ld];
    rules.extend(candidates::visible_price());
    rules.into_iter().find(|rule| {
        let mut any_value = false;
        let fits = pages
            .iter()
            .zip(&json_ld_prices)
            .all(|(page, json_ld_price)| {
                let Some(value) = page
                    .values(rule)
                    .and_then(|values| values.into_iter().next())
                else {
                    return !page.shown.price_text;
                };
                any_value = true;
                let reads_json_ld = matches!(rule.extract, ExtractionKind::JsonLdOffer { .. });
                let shows_its_amount = shows_amount(&page.html, &value);
                let agrees_with_json_ld = json_ld_price
                    .as_deref()
                    .is_none_or(|stated| same_amount(stated, &value));
                if reads_json_ld {
                    shows_its_amount
                } else {
                    is_price_text(&value) && agrees_with_json_ld
                }
            });
        fits && any_value
    })
}

/// The JSON-LD availability where it resolves on every page and no visible label contradicts it;
/// else the first visible label or cart action on every page that is no bare price.
fn draft_state(pages: &[DraftPage]) -> Option<ExtractionRule> {
    let visible = candidates::visible_state();
    let visible_readings: Vec<Option<StateReading>> = pages
        .iter()
        .map(|page| {
            visible.iter().find_map(|rule| {
                page.values(rule)
                    .and_then(|values| values.into_iter().next())
                    .map(|value| StateReading::of(&value))
            })
        })
        .collect();
    let json_ld = candidates::json_ld_availability();
    let json_ld_fits = pages.iter().zip(&visible_readings).all(|(page, shown)| {
        page.values(&json_ld)
            .and_then(|values| values.into_iter().next())
            .map(|value| StateReading::of(&value))
            .is_some_and(|stated| stated.resolved.is_some() && agree(stated, *shown))
    });
    if json_ld_fits {
        return Some(json_ld);
    }
    first_fitting(pages, visible, |_, values| {
        values
            .and_then(<[String]>::first)
            .is_some_and(|value| !is_bare_price(value))
    })
}

/// Whether a JSON-LD availability agrees with what the page visibly shows: neither says sold or
/// unavailable while the other says in stock.
fn agree(stated: StateReading, shown: Option<StateReading>) -> bool {
    let Some(shown) = shown else {
        return true;
    };
    let in_stock = |reading: StateReading| {
        matches!(
            reading.resolved,
            Some(
                ListingAvailability::Available
                    | ListingAvailability::InStock
                    | ListingAvailability::LimitedAvailability
            )
        )
    };
    !(in_stock(stated) && shown.shows_sold_or_unavailable()
        || stated.shows_sold_or_unavailable() && in_stock(shown))
}

fn is_bare_price(value: &str) -> bool {
    is_price_text(value) && StateReading::of(value) == StateReading::of("")
}

/// A product's own ID on every page, a different one on each: a value two pages share names a
/// template or the shop, and one that names an availability, such as a SKU a shop writes as
/// `35616 - SOLD` once the item sells, changes with it. With one page nothing shows it differs, so
/// no ID is drafted.
fn draft_source_listing_id(pages: &[DraftPage]) -> Option<ExtractionRule> {
    if pages.len() < 2 {
        return None;
    }
    candidates::source_listing_id().into_iter().find(|rule| {
        let ids: Option<Vec<String>> = pages.iter().map(|page| page.identity(rule)).collect();
        ids.is_some_and(|ids| {
            ids.iter().collect::<HashSet<_>>().len() == ids.len()
                && !ids.iter().any(|id| names_an_availability(id))
        })
    })
}

fn names_an_availability(value: &str) -> bool {
    let reading = StateReading::of(value);
    reading.resolved.is_some() || reading.mentioned.is_some()
}

/// Prose on every page that shows a description block, holding the start of the page's JSON-LD
/// description where it has one, so a shortened JSON-LD text bounds the visible one from below.
fn draft_description(pages: &[DraftPage]) -> Option<ExtractionRule> {
    first_fitting(pages, candidates::description(), |page, values| {
        let Some(parts) = values else {
            return !page.shown.description_block;
        };
        let text = collapsed(&parts.join(" "));
        let is_prose = text.chars().count() >= MIN_DESCRIPTION_CHARS
            && text.split_whitespace().count() >= MIN_DESCRIPTION_WORDS;
        is_prose
            && page
                .reference
                .description
                .as_deref()
                .is_none_or(|stated| contains_start_of(&text, stated))
    })
}

/// A gallery on every page with at least as many images as the page's JSON-LD names, and no image
/// every page shows: one photo never shows two products, so a URL on every page is a placeholder,
/// a lazy-loading sticker or the shop's logo, and a rule that takes it takes more than the gallery.
fn draft_images(pages: &[DraftPage]) -> Option<ExtractionRule> {
    candidates::images().into_iter().find(|rule| {
        let galleries: Option<Vec<Vec<String>>> = pages
            .iter()
            .map(|page| {
                page.values(rule)
                    .filter(|images| images.len() >= page.reference.image_count.max(1))
                    .map(|images| images.iter().map(|group| first_image(group)).collect())
            })
            .collect();
        galleries.is_some_and(|galleries| !shares_an_image(&galleries))
    })
}

/// The first URL of an image candidate group, as capture keeps it.
fn first_image(group: &str) -> String {
    split_image_candidate_group(group)
        .into_iter()
        .next()
        .unwrap_or(group)
        .to_owned()
}

/// Whether one image is in every gallery of two or more pages.
fn shares_an_image(galleries: &[Vec<String>]) -> bool {
    let [first, rest @ ..] = galleries else {
        return false;
    };
    !rest.is_empty()
        && first
            .iter()
            .any(|image| rest.iter().all(|gallery| gallery.contains(image)))
}

fn distinct_across_pages(values: &[String]) -> bool {
    values.len() < 2 || values.iter().collect::<HashSet<_>>().len() > 1
}

fn same_amount(stated: &str, shown: &str) -> bool {
    let whole = |price: &str| -> String {
        let number = price.split_whitespace().next().unwrap_or_default();
        number
            .split(['.', ','])
            .next()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_digit)
            .collect()
    };
    let stated = whole(stated);
    let shown: String = shown.chars().filter(char::is_ascii_digit).collect();
    !stated.is_empty() && shown.starts_with(&stated)
}

fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn contains_start_of(text: &str, stated: &str) -> bool {
    let fold = |text: &str| -> String {
        text.chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let start: String = fold(stated)
        .chars()
        .take(DESCRIPTION_REFERENCE_CHARS)
        .collect();
    fold(text).contains(&start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::JsonLdOfferProperty;

    const DESCRIPTION: &str = "A George III mahogany chest of drawers with four graduated long drawers, brass handles and bracket feet, circa 1790.";

    struct Page {
        title: &'static str,
        id: &'static str,
        json_ld: String,
        body: String,
    }

    impl Page {
        fn html(&self) -> String {
            format!(
                r#"<html><head><script type="application/ld+json">{}</script></head><body><div class="product"><h1 class="product_title">{}</h1><input type="hidden" name="product_id" value="{}">{}</div></body></html>"#,
                self.json_ld, self.title, self.id, self.body
            )
        }
    }

    fn woo_page(title: &'static str, id: &'static str, availability: &str, stock: &str) -> Page {
        let key = image_key(title);
        Page {
            title,
            id,
            json_ld: format!(
                r#"{{"@context":"https://schema.org","@type":"Product","name":"{title}","description":"{DESCRIPTION}","image":["https://shop.example/{key}-a.jpg","https://shop.example/{key}-b.jpg"],"offers":{{"@type":"Offer","price":"1250.00","priceCurrency":"GBP","availability":"https://schema.org/{availability}"}}}}"#
            ),
            body: format!(
                r#"<div class="woocommerce-product-gallery__image"><img src="https://shop.example/{key}-a.jpg"></div><div class="woocommerce-product-gallery__image"><img src="https://shop.example/{key}-b.jpg"></div><div class="summary"><p class="price">£1,250.00</p><div class="woocommerce-product-details__short-description"><p>{DESCRIPTION} Restored by our workshop.</p></div><p class="stock">{stock}</p></div>"#
            ),
        }
    }

    /// The image name of a product, one per title, so no two products share a photo.
    fn image_key(title: &str) -> String {
        title.replace(' ', "-")
    }

    fn draft(pages: &[Page]) -> Option<StructuredDraft> {
        let html: Vec<String> = pages.iter().map(Page::html).collect();
        draft_schema(html.iter().map(String::as_str))
    }

    fn selector(rule: Option<&ExtractionRule>) -> Option<String> {
        rule.map(|rule| rule.selector.to_string())
    }

    #[test]
    fn should_draft_every_field_from_agreeing_json_ld_and_visible_markup() {
        let draft = draft(&[
            woo_page("Chest of drawers", "101", "InStock", "1 in stock"),
            woo_page("Mahogany chest", "102", "OutOfStock", "Out of stock"),
        ])
        .expect("draft");

        assert!(draft.is_complete(), "{:?}", draft.undrafted);
        let schema = &draft.schema;
        assert_eq!(
            selector(schema.title.as_ref()).as_deref(),
            Some("h1.product_title")
        );
        assert_eq!(
            schema.price.as_ref().map(|rule| rule.extract.clone()),
            Some(ExtractionKind::JsonLdOffer {
                property: JsonLdOfferProperty::Price
            })
        );
        assert_eq!(
            schema.state.as_ref().map(|rule| rule.extract.clone()),
            Some(ExtractionKind::JsonLdOffer {
                property: JsonLdOfferProperty::Availability
            })
        );
        assert_eq!(
            selector(schema.source_listing_id.as_ref()).as_deref(),
            Some("input[name='product_id']")
        );
        assert!(schema.description.is_some());
        assert!(schema.images.is_some());
        assert!(ProductCssSelectorSchema::validate_all(std::slice::from_ref(schema)).is_ok());
    }

    #[test]
    fn should_read_the_visible_state_when_the_json_ld_says_in_stock_on_a_sold_page() {
        let draft = draft(&[
            woo_page("Chest of drawers", "101", "InStock", "1 in stock"),
            woo_page("Mahogany chest", "102", "InStock", "Out of stock"),
        ])
        .expect("draft");

        assert_eq!(
            selector(draft.schema.state.as_ref()).as_deref(),
            Some(".summary p.stock")
        );
    }

    #[test]
    fn should_read_the_visible_price_when_the_page_does_not_show_the_json_ld_amount() {
        let mut page = woo_page("Chest of drawers", "101", "InStock", "1 in stock");
        page.json_ld = page.json_ld.replace("1250.00", "999.00");
        let other = woo_page("Mahogany chest", "102", "InStock", "1 in stock");

        let draft = draft(&[page, other]).expect("draft");

        assert!(draft.schema.price.is_none());
        assert_eq!(draft.undrafted, vec![UndraftedField::Price]);
    }

    #[test]
    fn should_leave_the_description_to_the_author_when_the_visible_text_lacks_the_json_ld_one() {
        let mut page = woo_page("Chest of drawers", "101", "InStock", "1 in stock");
        page.body = page.body.replace(DESCRIPTION, "Short label");
        let other = woo_page("Mahogany chest", "102", "InStock", "1 in stock");

        let draft = draft(&[page, other]).expect("draft");

        assert!(draft.schema.description.is_none());
        assert!(draft.undrafted.contains(&UndraftedField::Description));
    }

    #[test]
    fn should_leave_the_images_to_the_author_when_the_gallery_shows_fewer_than_the_json_ld() {
        let pages: Vec<Page> = [("Chest of drawers", "101"), ("Mahogany chest", "102")]
            .into_iter()
            .map(|(title, id)| {
                let key = image_key(title);
                let mut page = woo_page(title, id, "InStock", "1 in stock");
                page.json_ld = page.json_ld.replace(
                    &format!(r#""https://shop.example/{key}-b.jpg""#),
                    &format!(
                        r#""https://shop.example/{key}-b.jpg","https://shop.example/{key}-c.jpg""#
                    ),
                );
                page
            })
            .collect();

        let draft = draft(&pages).expect("draft");

        assert!(draft.schema.images.is_none());
        assert!(draft.undrafted.contains(&UndraftedField::Images));
    }

    #[test]
    fn should_leave_the_images_to_the_author_when_every_gallery_holds_the_same_placeholder() {
        let pages: Vec<Page> = [("Chest of drawers", "101"), ("Mahogany chest", "102")]
            .into_iter()
            .map(|(title, id)| {
                let key = image_key(title);
                let mut page = woo_page(title, id, "InStock", "1 in stock");
                page.body = page.body.replace(
                    &format!(r#"<img src="https://shop.example/{key}-b.jpg">"#),
                    &format!(r#"<img src="https://shop.example/{key}-b.jpg"></div><div class="woocommerce-product-gallery__image"><img src="https://shop.example/loading.png">"#),
                );
                page
            })
            .collect();

        let draft = draft(&pages).expect("draft");

        assert!(draft.schema.images.is_none());
        assert!(draft.undrafted.contains(&UndraftedField::Images));
    }

    #[test]
    fn should_count_an_image_the_json_ld_lists_twice_once() {
        let pages: Vec<Page> = [("Chest of drawers", "101"), ("Mahogany chest", "102")]
            .into_iter()
            .map(|(title, id)| {
                let key = image_key(title);
                let mut page = woo_page(title, id, "InStock", "1 in stock");
                page.json_ld = page.json_ld.replace(
                    &format!(r#""https://shop.example/{key}-b.jpg""#),
                    &format!(r#""https://shop.example/{key}-b.jpg",{{"@type":"ImageObject","url":"https://shop.example/{key}-a.jpg"}}"#),
                );
                page
            })
            .collect();

        let draft = draft(&pages).expect("draft");

        assert!(draft.schema.images.is_some(), "{:?}", draft.undrafted);
    }

    #[test]
    fn should_draft_no_identity_that_two_pages_share() {
        let draft = draft(&[
            woo_page("Chest of drawers", "7", "InStock", "1 in stock"),
            woo_page("Mahogany chest", "7", "InStock", "1 in stock"),
        ])
        .expect("draft");

        assert!(draft.schema.source_listing_id.is_none());
        assert!(draft.is_complete());
    }

    #[test]
    fn should_draft_no_identity_that_names_the_product_s_availability() {
        let draft = draft(&[
            woo_page("Chest of drawers", "35351", "InStock", "1 in stock"),
            woo_page(
                "Mahogany chest",
                "35616 - SOLD",
                "OutOfStock",
                "Out of stock",
            ),
        ])
        .expect("draft");

        assert!(draft.schema.source_listing_id.is_none());
    }

    #[test]
    fn should_draft_nothing_without_a_title_on_every_page() {
        let mut page = woo_page("Chest of drawers", "101", "InStock", "1 in stock");
        page.title = "";

        assert!(draft(&[page]).is_none());
    }
}
