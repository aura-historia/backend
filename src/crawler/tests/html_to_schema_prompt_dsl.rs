use crawler::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, RawExtractedProduct,
};
use crawler::scraper::css_selector::product_schema_service::{
    clean_html_for_schema_generation, html_to_schema_prompt_dsl,
};
use crawler::scraper::css_selector::rule::ExtractionRule;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Deserialize)]
struct FixtureJson {
    html: String,
    schema: ProductCssSelectorSchema,
    raw: RawExpectation,
}

#[derive(Debug, Deserialize)]
struct RawExpectation {
    shops_product_id: String,
    title: String,
    description: Vec<String>,
    price: Option<String>,
    price_estimate_min: Option<String>,
    price_estimate_max: Option<String>,
    seller_name: Option<String>,
    state: String,
    images: Vec<String>,
    auction_start: Option<String>,
    auction_end: Option<String>,
}

#[test]
fn should_project_all_html_fixtures_to_compact_schema_prompt_dsl() {
    for fixture in load_fixtures() {
        let html = read_fixture_html(&fixture.html);
        let cleaned_html = clean_html_for_schema_generation(&html);
        let dsl = html_to_schema_prompt_dsl(&html);

        assert!(
            !dsl.trim().is_empty(),
            "{} should produce non-empty DSL",
            fixture.html
        );
        assert!(
            dsl.len() < html.len(),
            "{} DSL should be smaller than raw HTML: dsl={} html={}",
            fixture.html,
            dsl.len(),
            html.len()
        );
        assert!(
            dsl.len() < cleaned_html.len(),
            "{} YAML DSL should be smaller than cleaned HTML: yaml_dsl={} cleaned_html={} raw_html={}",
            fixture.html,
            dsl.len(),
            cleaned_html.len(),
            html.len()
        );
        assert_no_raw_noise_blocks(&fixture.html, &dsl);
        assert_schema_selectors_are_represented(&fixture.html, &dsl, &fixture.schema);
        assert_raw_expectations_are_represented(&fixture.html, &dsl, &fixture.raw);
        assert_fixture_specific_expectations(&fixture.html, &dsl);
    }
}

#[test]
#[ignore = "diagnostic size report for prompt-cost analysis"]
fn should_print_schema_prompt_dsl_size_report() {
    let mut totals = SizeReportRow::default();

    println!(
        "{:<34} {:>12} {:>14} {:>12} {:>10} {:>13} {:>12} {:>14} {:>12}",
        "fixture",
        "raw_bytes",
        "cleaned_bytes",
        "yaml_bytes",
        "yaml/raw",
        "yaml/cleaned",
        "raw_tokens",
        "cleaned_tokens",
        "yaml_tokens"
    );

    for fixture in load_fixtures() {
        let html = read_fixture_html(&fixture.html);
        let cleaned_html = clean_html_for_schema_generation(&html);
        let dsl = html_to_schema_prompt_dsl(&html);
        let row = SizeReportRow {
            raw_bytes: html.len(),
            cleaned_bytes: cleaned_html.len(),
            yaml_bytes: dsl.len(),
        };
        totals.add(row);

        print_size_report_row(
            fixture
                .html
                .strip_prefix("tests/fixtures/html/")
                .unwrap_or(&fixture.html),
            row,
        );
    }

    print_size_report_row("TOTAL", totals);
}

#[tokio::test]
#[ignore = "one-off diagnostic size report for 20th Century Militaria"]
async fn should_print_20th_century_militaria_schema_prompt_dsl_size_report() {
    let fetcher = live_benchmark_client();
    let url = "https://20thcenturymilitaria.com/shop.php?code=52011";
    let html = fetch_live_html(&fetcher, url).await;
    let cleaned_html = clean_html_for_schema_generation(&html);
    let dsl = html_to_schema_prompt_dsl(&html);
    let row = SizeReportRow {
        raw_bytes: html.len(),
        cleaned_bytes: cleaned_html.len(),
        yaml_bytes: dsl.len(),
    };

    println!(
        "{:<34} {:>12} {:>14} {:>12} {:>10} {:>13} {:>12} {:>14} {:>12}",
        "live_product",
        "raw_bytes",
        "cleaned_bytes",
        "yaml_bytes",
        "yaml/raw",
        "yaml/cleaned",
        "raw_tokens",
        "cleaned_tokens",
        "yaml_tokens"
    );
    print_size_report_row("20th-century-militaria/52011", row);

    for expected in [
        "WW2 MkII Helmet",
        "Royal Marines",
        "52011",
        "shop.php?code=52011",
    ] {
        assert!(
            dsl.contains(expected),
            "20th Century Militaria YAML should preserve {expected:?}"
        );
    }
}

#[tokio::test]
#[ignore = "live network benchmark for prompt-cost and data-preservation analysis"]
async fn should_print_live_shop_product_schema_prompt_dsl_size_report() {
    let fixtures = load_fixtures();
    let fetcher = live_benchmark_client();
    let mut totals = SizeReportRow::default();

    println!(
        "{:<34} {:>12} {:>14} {:>12} {:>10} {:>13} {:>12} {:>14} {:>12}",
        "live_product",
        "raw_bytes",
        "cleaned_bytes",
        "yaml_bytes",
        "yaml/raw",
        "yaml/cleaned",
        "raw_tokens",
        "cleaned_tokens",
        "yaml_tokens"
    );

    for shop in live_shop_benchmark_cases() {
        assert_eq!(
            shop.urls.len(),
            5,
            "{} benchmark should use five live product pages",
            shop.name
        );

        let fixture = fixtures
            .iter()
            .find(|fixture| fixture.html.ends_with(shop.fixture_file))
            .unwrap_or_else(|| panic!("missing fixture schema for {}", shop.fixture_file));

        let mut shop_totals = SizeReportRow::default();
        let mut shop_quality_checked_pages = 0;
        let mut shop_schema_failures = 0;
        for url in shop.urls {
            let html = fetch_live_html(&fetcher, url).await;
            let cleaned_html = clean_html_for_schema_generation(&html);
            let dsl = html_to_schema_prompt_dsl(&html);

            assert!(
                !dsl.trim().is_empty(),
                "{} live product {url} should produce non-empty DSL",
                shop.name
            );
            assert!(
                dsl.len() < cleaned_html.len(),
                "{} live product {url} YAML DSL should be smaller than cleaned HTML: yaml_dsl={} cleaned_html={} raw_html={}",
                shop.name,
                dsl.len(),
                cleaned_html.len(),
                html.len()
            );
            assert_no_raw_noise_blocks(url, &dsl);

            let row = SizeReportRow {
                raw_bytes: html.len(),
                cleaned_bytes: cleaned_html.len(),
                yaml_bytes: dsl.len(),
            };
            shop_totals.add(row);
            totals.add(row);
            print_size_report_row(&live_report_label(shop.name, url), row);

            let parsed_html = ::scraper::Html::parse_document(&html);
            match fixture.schema.apply(&parsed_html) {
                Ok(raw) => {
                    shop_quality_checked_pages += 1;
                    assert_live_raw_extraction_is_represented(shop.name, url, &dsl, &raw);
                }
                Err(err) => {
                    shop_schema_failures += 1;
                    println!(
                        "QUALITY-SKIP {:<16} {} fixture schema did not extract this live page: {}",
                        shop.name, url, err
                    );
                }
            }
        }

        print_size_report_row(&format!("{} TOTAL", shop.name), shop_totals);
        println!(
            "QUALITY {:<16} checked={} schema_failures={}",
            shop.name, shop_quality_checked_pages, shop_schema_failures
        );
        assert!(
            shop_quality_checked_pages > 0,
            "{} should have at least one live page where the fixture schema can validate data preservation",
            shop.name
        );
    }

    print_size_report_row("LIVE TOTAL", totals);
}

#[derive(Debug)]
struct LiveShopBenchmarkCase {
    name: &'static str,
    fixture_file: &'static str,
    urls: &'static [&'static str],
}

fn live_shop_benchmark_cases() -> Vec<LiveShopBenchmarkCase> {
    vec![
        LiveShopBenchmarkCase {
            name: "weitze",
            fixture_file: "weitze_available.html",
            urls: &[
                "https://www.weitze.net/militaria/00/Reichswehr_Wehrmacht_Paar_Schulterknoepfe_fuer_einen_Soldaten_der_6_Kompanie__405500.html",
                "https://www.weitze.net/militaria/00/Wehrmacht_Fangschnur_fuer_den_Streifendienst__356500.html",
                "https://www.weitze.net/militaria/00/Wehrmacht_Heer_Aermelabzeichen_Mannschaft_Sanitaeter__470600.html",
                "https://www.weitze.net/militaria/00/Wehrmacht_Heer_Aermelabzeichen_Oberschuetzenstern__463100.html",
                "https://www.weitze.net/militaria/00/Wehrmacht_Heer_Aermelband_des_Infanterie_Regiment_Nr_271_Feldherrnhalle_fuer_Offiziere__504400.html",
            ],
        },
        LiveShopBenchmarkCase {
            name: "chairish",
            fixture_file: "chairish_available.html",
            urls: &[
                "https://www.chairish.com/product/26206792/tall-pair-of-palm-beach-style-palm-tree-candle-holders",
                "https://www.chairish.com/product/32347687/antique-brass-candle-holders",
                "https://www.chairish.com/product/23974836/midcentury-modern-sculptural-brass-candle-holder-candles-included",
                "https://www.chairish.com/product/34043451/vintage-brass-candle-holder-4-tier-17-candle-holders",
                "https://www.chairish.com/product/6329166/modern-solid-wood-pillar-candle-holders-a-pair",
            ],
        },
        LiveShopBenchmarkCase {
            name: "lot-tissimo",
            fixture_file: "lot-tissimo_listed.html",
            urls: &[
                "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd",
                "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a34c4335-19e6-4a6d-808f-b3fd00bbfcc7",
                "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-6803a25f-7f6a-4ef0-a614-b3fd00bae904",
                "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-2583b894-78e6-4b29-a97b-b3fd00b9b0f5",
                "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-835b5a20-6e4f-4b9b-b9c9-b3fd00ba6b65",
            ],
        },
        LiveShopBenchmarkCase {
            name: "antik-und-stil",
            fixture_file: "antik-und-stil_available.html",
            urls: &[
                "https://antik-und-stil.com/produkt/antiker-tisch-rund-marmor-esstisch-mit-schubladen/",
                "https://antik-und-stil.com/produkt/6-antike-stuehle-eiche-um-1900-esszimmerstuehle-restauriert/",
                "https://antik-und-stil.com/produkt/antike-garderobe-jugendstil-eiche-um-1900-mit-spiegel/",
                "https://antik-und-stil.com/produkt/antike-jugendstil-vitrine-um-1910-buecherschrank-mit-spiegelrueckwand/",
                "https://antik-und-stil.com/produkt/antiker-esstisch-eiche-massiv-180-cm-kulissentisch-mit-schublade/",
            ],
        },
    ]
}

fn live_benchmark_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("curl/8.13.0")
        .build()
        .expect("live benchmark HTTP client should build")
}

async fn fetch_live_html(fetcher: &reqwest::Client, url: &str) -> String {
    let url =
        Url::parse(url).unwrap_or_else(|err| panic!("invalid live benchmark URL {url}: {err}"));
    fetcher
        .get(url.clone())
        .send()
        .await
        .unwrap_or_else(|err| panic!("failed fetching live benchmark URL {url}: {err}"))
        .error_for_status()
        .unwrap_or_else(|err| panic!("live benchmark URL {url} returned an error status: {err}"))
        .text()
        .await
        .unwrap_or_else(|err| panic!("failed reading live benchmark URL {url} response: {err}"))
}

fn live_report_label(shop: &str, url: &str) -> String {
    let url = url.trim_end_matches('/');
    let slug = url
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .chars()
        .take(20)
        .collect::<String>();
    format!("{shop}/{slug}")
}

#[derive(Debug, Clone, Copy, Default)]
struct SizeReportRow {
    raw_bytes: usize,
    cleaned_bytes: usize,
    yaml_bytes: usize,
}

impl SizeReportRow {
    fn add(&mut self, row: SizeReportRow) {
        self.raw_bytes += row.raw_bytes;
        self.cleaned_bytes += row.cleaned_bytes;
        self.yaml_bytes += row.yaml_bytes;
    }
}

fn print_size_report_row(label: &str, row: SizeReportRow) {
    println!(
        "{:<34} {:>12} {:>14} {:>12} {:>9.1}% {:>12.1}% {:>12} {:>14} {:>12}",
        label,
        row.raw_bytes,
        row.cleaned_bytes,
        row.yaml_bytes,
        percent(row.yaml_bytes, row.raw_bytes),
        percent(row.yaml_bytes, row.cleaned_bytes),
        approx_tokens(row.raw_bytes),
        approx_tokens(row.cleaned_bytes),
        approx_tokens(row.yaml_bytes)
    );
}

fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

fn approx_tokens(chars: usize) -> usize {
    chars / 4
}

fn load_fixtures() -> Vec<FixtureJson> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fixtures.json");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed reading '{}': {err}", path.display()));
    serde_json::from_str(&src)
        .unwrap_or_else(|err| panic!("failed parsing '{}': {err}", path.display()))
}

fn read_fixture_html(path: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed reading '{}': {err}", path.display()))
}

fn assert_no_raw_noise_blocks(fixture_path: &str, dsl: &str) {
    for needle in [
        "script", "style", "svg", "canvas", "nav", "header", "footer", "aside",
    ] {
        assert!(
            !dsl.contains(&format!("<{needle}")) && !dsl.contains(&format!("tag: {needle}")),
            "{fixture_path} DSL should not contain noise block {needle}"
        );
    }
}

fn assert_schema_selectors_are_represented(
    fixture_path: &str,
    dsl: &str,
    schema: &ProductCssSelectorSchema,
) {
    for (field, rule) in schema_rules(schema) {
        assert_selector_is_represented(fixture_path, field, rule.selector.as_ref(), dsl);
        for selector in &rule.additional_selectors {
            assert_selector_is_represented(fixture_path, field, selector.as_ref(), dsl);
        }
    }
}

fn schema_rules(schema: &ProductCssSelectorSchema) -> Vec<(&'static str, &ExtractionRule)> {
    let mut rules = vec![
        ("shops_product_id", &schema.shops_product_id),
        ("title", &schema.title),
        ("state", &schema.state),
        ("images", &schema.images),
    ];
    if let Some(rule) = &schema.description {
        rules.push(("description", rule));
    }
    if let Some(rule) = &schema.price {
        rules.push(("price", rule));
    }
    if let Some(rule) = &schema.price_estimate_min {
        rules.push(("price_estimate_min", rule));
    }
    if let Some(rule) = &schema.price_estimate_max {
        rules.push(("price_estimate_max", rule));
    }
    if let Some(rule) = &schema.seller_name {
        rules.push(("seller_name", rule));
    }
    if let Some(rule) = &schema.auction_start {
        rules.push(("auction_start", rule));
    }
    if let Some(rule) = &schema.auction_end {
        rules.push(("auction_end", rule));
    }
    rules
}

fn assert_selector_is_represented(fixture_path: &str, field: &str, selector: &str, dsl: &str) {
    assert!(
        selector_represented(selector, dsl),
        "{fixture_path} DSL should represent {field} selector {selector:?}"
    );
}

fn selector_represented(selector: &str, dsl: &str) -> bool {
    if dsl.contains(selector) {
        return true;
    }

    selector
        .split_whitespace()
        .any(|part| selector_part_represented(part, dsl))
}

fn selector_part_represented(part: &str, dsl: &str) -> bool {
    if dsl.contains(part) {
        return true;
    }

    let tokens = selector_probe_tokens(part);
    !tokens.is_empty() && tokens.iter().any(|token| dsl.contains(token))
}

fn selector_probe_tokens(selector_part: &str) -> Vec<&str> {
    selector_part
        .split([
            '.', '#', '[', ']', '=', '\'', '"', '>', '+', '~', ':', ',', '(', ')',
        ])
        .filter(|token| token.len() > 1)
        .filter(|token| !matches!(*token, "nth-child" | "nth-of-type"))
        .collect()
}

fn assert_raw_expectations_are_represented(fixture_path: &str, dsl: &str, raw: &RawExpectation) {
    assert_value_is_represented(fixture_path, "shops_product_id", &raw.shops_product_id, dsl);
    assert_value_is_represented(fixture_path, "title", &raw.title, dsl);
    assert_value_is_represented(fixture_path, "state", &raw.state, dsl);
    assert_optional_value_is_represented(fixture_path, "price", raw.price.as_deref(), dsl);
    assert_optional_value_is_represented(
        fixture_path,
        "price_estimate_min",
        raw.price_estimate_min.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(
        fixture_path,
        "price_estimate_max",
        raw.price_estimate_max.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(
        fixture_path,
        "seller_name",
        raw.seller_name.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(
        fixture_path,
        "auction_start",
        raw.auction_start.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(
        fixture_path,
        "auction_end",
        raw.auction_end.as_deref(),
        dsl,
    );
    if let Some(description) = raw.description.first() {
        assert_value_is_represented(fixture_path, "description", description, dsl);
    }
    if let Some(image) = raw.images.first() {
        assert_value_is_represented(fixture_path, "image", image, dsl);
    }
}

fn assert_live_raw_extraction_is_represented(
    shop: &str,
    url: &str,
    dsl: &str,
    raw: &RawExtractedProduct,
) {
    let context = format!("{shop} live product {url}");
    assert_value_is_represented(&context, "shops_product_id", &raw.shops_product_id, dsl);
    assert_value_is_represented(&context, "title", &raw.title, dsl);
    assert_value_is_represented(&context, "state", &raw.state, dsl);
    assert_optional_value_is_represented(&context, "price", raw.price.as_deref(), dsl);
    assert_optional_value_is_represented(
        &context,
        "price_estimate_min",
        raw.price_estimate_min.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(
        &context,
        "price_estimate_max",
        raw.price_estimate_max.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(&context, "seller_name", raw.seller_name.as_deref(), dsl);
    assert_optional_value_is_represented(
        &context,
        "auction_start",
        raw.auction_start.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(&context, "auction_end", raw.auction_end.as_deref(), dsl);
    if let Some(description) = raw.description.first() {
        assert_value_is_represented(&context, "description", description, dsl);
    }
    if let Some(image) = raw.images.first() {
        assert_value_is_represented(&context, "image", image, dsl);
    }
}

fn assert_optional_value_is_represented(
    fixture_path: &str,
    field: &str,
    value: Option<&str>,
    dsl: &str,
) {
    if let Some(value) = value {
        assert_value_is_represented(fixture_path, field, value, dsl);
    }
}

fn assert_value_is_represented(fixture_path: &str, field: &str, value: &str, dsl: &str) {
    let value = normalize_probe_text(value);
    if value.is_empty() {
        return;
    }
    let probe = value.chars().take(180).collect::<String>();
    let normalized_dsl = normalize_probe_text(dsl);
    let compact_probe = compact_probe_text(&probe);
    let compact_dsl = compact_probe_text(&normalized_dsl);
    assert!(
        normalized_dsl.contains(&probe)
            || compact_dsl.contains(&compact_probe)
            || significant_words_are_represented(&probe, &normalized_dsl),
        "{fixture_path} DSL should contain expected raw {field} value {probe:?}"
    );
}

fn normalize_probe_text(value: &str) -> String {
    value
        .replace("\\u2026", "...")
        .replace('…', "...")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_probe_text(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn significant_words_are_represented(probe: &str, dsl: &str) -> bool {
    let words = probe
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() > 2)
        .take(8)
        .collect::<Vec<_>>();

    !words.is_empty() && words.iter().all(|word| dsl.contains(word))
}

fn assert_fixture_specific_expectations(fixture_path: &str, dsl: &str) {
    match fixture_path {
        path if path.contains("lot-tissimo") => {
            for needle in [
                "id: LotId",
                "id: ClientName",
                "id: lot-is-ended",
                "property: og:title",
                "property: og:description",
                "Kunstauktionshaus Leipzig",
                "cdn.globalauctionplatform.com",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("chairish") => {
            for needle in [
                "property: og:product-id",
                "property: product:availability",
                "class: product-title",
                "js-product-description",
                "tag: img",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("weitze") => {
            for needle in [
                "tag: h1",
                "itemprop: name",
                "tag: span",
                "itemprop: price",
                "itemprop: image",
                "405500",
                "25,00",
                "405500.webp",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("antik-und-stil") => {
            for needle in [
                "sku",
                "product_title",
                "single_add_to_cart_button",
                "tag: img",
                "2025-0219-03",
                "Tisch rund antik Marmorplatte",
                "In den Warenkorb",
                "Tisch-Marmor1000",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        _ => {}
    }
}
