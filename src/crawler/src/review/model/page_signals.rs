use super::SchemaPageReference;
use serde::{Deserialize, Serialize};

/// What a review page shows that a candidate schema should capture, read from the page's markup
/// when the matrix evaluated it, so the [approval guard](crate::review::approval_guard) can tell
/// a missing `price` or `description` rule from a page that has neither. Heuristics: they decide
/// nothing alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSignals {
    pub page_reference: SchemaPageReference,
    pub url: String,
    /// A price element of the page's own content holds an amount with its currency, or the offers
    /// of the page's own JSON-LD item state a price above zero.
    pub shows_price_text: bool,
    /// A description element of the page's own content holds a block of prose.
    pub shows_description_block: bool,
}
