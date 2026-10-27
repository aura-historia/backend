use crate::scraper::css_selector::product_schema::{
    ExtractionLogging, ProductCssSelectorSchema, RawExtractedProduct,
};
use sha2::{Digest, Sha256};

/// Returns the page hash: the SHA-256 hex digest of what each schema of `schemas`, in order,
/// extracts from `html`, or of the fact that it does not apply.
///
/// It covers exactly what extraction reads, wherever it is on the page. Content no schema
/// extracts, such as the form tokens, nonces, session ids in links and randomised form labels
/// some shops render on every request, leaves it unchanged; a change to any value a schema
/// extracts changes it (`docs/crawler/extraction-and-review.md#extraction-contract`). It
/// extracts silently, so it adds no log line to a scrape.
pub(crate) fn hash_page(html: &str, schemas: &[ProductCssSelectorSchema]) -> String {
    let document = scraper::Html::parse_document(html);
    let mut page = Sha256::new();
    for schema in schemas {
        match schema.extract(&document, ExtractionLogging::Silent) {
            Ok(extraction) => {
                page.update([1]);
                hash_extraction(&mut page, &extraction.raw);
            }
            Err(_) => page.update([0]),
        }
    }
    hex(&page.finalize())
}

/// Version of the scrape behaviour that turns the schemas' raw extraction of a page into the
/// captured values, such as image validation and the runtime checks that select a schema.
///
/// It is part of the scraper-context fingerprint, so raising it extracts every page once more at
/// its next scrape although neither its page hash nor its schemas changed. Raise it with every
/// change that alters the captured values of an unchanged raw extraction; a change to raw
/// extraction itself changes the page hash of every page it affects and needs no raise
/// (`docs/crawler/extraction-and-review.md#extraction-contract`).
pub(crate) const EXTRACTOR_VERSION: u32 = 1;

/// Fingerprint the extractor version, the ordered effective schema set and the source fallback
/// currency. Schema structures serialize deterministically (raw keys are a `BTreeMap`), so an
/// extractor-version, selector or fallback-currency change forces extraction even for unchanged
/// HTML.
pub(crate) fn fingerprint_scraper_context(
    extractor_version: u32,
    schemas: &[ProductCssSelectorSchema],
    fallback_currency: Option<money::Currency>,
) -> Result<String, serde_json::Error> {
    let encoded = serde_json::to_vec(&(
        extractor_version,
        schemas,
        fallback_currency.map(money::Currency::as_str),
    ))?;
    Ok(hex(&Sha256::digest(encoded)))
}

/// Feeds what one schema extracted to the page hash. Each value is framed by its length so that
/// values cannot run into each other, and the extraction is destructured so that a field added
/// to it cannot be left out of the hash.
fn hash_extraction(page: &mut Sha256, raw: &RawExtractedProduct) {
    let RawExtractedProduct {
        source_listing_id,
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
        state,
        images,
        raw_attributes,
    } = raw;
    hash_text(page, source_listing_id);
    hash_text(page, title);
    hash_texts(page, description);
    hash_optional_text(page, price.as_deref());
    hash_optional_text(page, price_estimate_min.as_deref());
    hash_optional_text(page, price_estimate_max.as_deref());
    hash_text(page, state);
    hash_texts(page, images);
    hash_len(page, raw_attributes.len());
    for (field, values) in raw_attributes {
        hash_text(page, field);
        hash_texts(page, values);
    }
}

fn hash_optional_text(page: &mut Sha256, value: Option<&str>) {
    match value {
        None => page.update([0]),
        Some(value) => {
            page.update([1]);
            hash_text(page, value);
        }
    }
}

fn hash_texts(page: &mut Sha256, values: &[String]) {
    hash_len(page, values.len());
    for value in values {
        hash_text(page, value);
    }
}

fn hash_text(page: &mut Sha256, value: &str) {
    hash_len(page, value.len());
    page.update(value.as_bytes());
}

fn hash_len(page: &mut Sha256, len: usize) {
    page.update((len as u64).to_le_bytes());
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use crate::scraper::scraper_service::extraction::engine::apply_schema_to_document;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    /// A product page with the per-request tokens shops render inside `<main>`: a checkout
    /// link's session id (Harrison Antique Furniture), and an enquiry form's anti-spam tokens
    /// and honeypot label, whose words WPForms shuffles per request (Collinge Antiques).
    fn page(price: &str, session_id: &str, form_token: &str, honeypot_label: &str) -> String {
        format!(
            r#"<html><body><main>
              <span class="checkout"><a href="/index.asp?function=CHECKOUT&uid={session_id}">Checkout</a></span>
              <h1>Walnut Dumb Waiter</h1>
              <span id="price">{price}</span>
              <span id="state">In Stock</span>
              <a class="gallery" href="/images/waiter-1800.jpg"><img src="/images/waiter-300.jpg"></a>
              <form data-token-time="{form_token}">
                <label for="wpforms-37-field_4">{honeypot_label}</label>
                <input type="text" id="wpforms-37-field_4" name="wpforms[fields][4]">
                <input type="hidden" name="wpforms[time_token]" value="{form_token}">
                <input type="hidden" name="wpforms[post_id]" value="47679">
                <button type="submit">Send</button>
              </form>
            </main></body></html>"#
        )
    }

    fn rule(selector: &str, extract: ExtractionKind) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: None,
            title: rule("h1", ExtractionKind::Text),
            description: None,
            price: Some(rule("#price", ExtractionKind::Text)),
            price_estimate_min: None,
            price_estimate_max: None,
            state: rule("#state", ExtractionKind::Text),
            images: ExtractionRule {
                cardinality: ExtractionCardinality::All,
                ..rule("a.gallery", ExtractionKind::ImageUrl)
            },
            raw_attributes: Default::default(),
        }
    }

    #[test]
    fn should_keep_the_page_hash_when_only_per_request_tokens_change() {
        let first = page("£795", "3C264776", "1793103358", "Subject Name Message");
        let second = page("£795", "B08072A2", "1793103362", "Address Name Subject");

        assert_eq!(
            hash_page(&first, &[schema()]),
            hash_page(&second, &[schema()])
        );
    }

    #[test]
    fn should_change_the_page_hash_when_an_extracted_value_changes() {
        let listed = page("£795", "3C264776", "1793103358", "Subject Name Message");
        let reduced = page("£695", "3C264776", "1793103358", "Subject Name Message");

        assert_ne!(
            hash_page(&listed, &[schema()]),
            hash_page(&reduced, &[schema()])
        );
    }

    #[test]
    fn should_change_the_page_hash_when_a_value_a_schema_reads_from_a_form_changes() {
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(rule(
                r#"input[name="wpforms[post_id]"]"#,
                ExtractionKind::Attribute {
                    name: "value".into(),
                },
            )),
            ..schema()
        };
        let page = page("£795", "3C264776", "1793103358", "Subject Name Message");
        let renumbered = page.replace(r#"value="47679""#, r#"value="47680""#);

        assert_ne!(
            hash_page(&page, std::slice::from_ref(&schema)),
            hash_page(&renumbered, &[schema])
        );
    }

    #[test]
    fn should_hash_a_page_without_logging_its_optional_field_misses() {
        struct CountEvents(Arc<AtomicUsize>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountEvents {
            fn on_event(&self, _: &tracing::Event<'_>, _: Context<'_, S>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let schema = ProductCssSelectorSchema {
            description: Some(rule("#description", ExtractionKind::Text)),
            ..schema()
        };
        let page = page("£795", "3C264776", "1793103358", "Subject Name Message");
        let events = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(CountEvents(Arc::clone(&events)));

        tracing::subscriber::with_default(subscriber, || {
            hash_page(&page, std::slice::from_ref(&schema));
            let logged_by_hash = events.load(Ordering::SeqCst);
            apply_schema_to_document(&schema, &scraper::Html::parse_document(&page))
                .unwrap_or_else(|error| panic!("the schema must apply: {error}"));

            assert_eq!(logged_by_hash, 0);
            assert!(
                events.load(Ordering::SeqCst) > 0,
                "the extraction a changed page goes on to still logs the miss"
            );
        });
    }

    #[test]
    fn should_change_the_page_hash_when_a_schema_stops_applying() {
        let page = page("£795", "3C264776", "1793103358", "Subject Name Message");
        let untitled = page.replace("<h1>Walnut Dumb Waiter</h1>", "");

        assert_ne!(
            hash_page(&page, &[schema()]),
            hash_page(&untitled, &[schema()])
        );
    }
}
