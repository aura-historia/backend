use scraper::Selector;
use std::sync::OnceLock;

static MAIN_SEL: OnceLock<Selector> = OnceLock::new();

fn main_selector() -> &'static Selector {
    MAIN_SEL.get_or_init(|| Selector::parse("main").expect("valid selector"))
}

pub(crate) fn extract_main_fragment(html: &str) -> Option<String> {
    let document = scraper::Html::parse_document(html);
    document
        .select(main_selector())
        .next()
        .map(|el| el.inner_html())
}
