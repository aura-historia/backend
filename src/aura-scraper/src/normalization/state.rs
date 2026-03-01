use common::product_state::domain::ProductState;
use once_cell::sync::OnceCell;
use regex::Regex;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Static lookup table
// ---------------------------------------------------------------------------

/// Maps trimmed, lower-cased raw state strings to a [`ProductState`].
///
/// This is the OnceCell-backed "database" that will later be replaced by a
/// real database call. If a value is not found here the service will fall back
/// to the regex patterns below, then to `ProductState::Unknown`.
static STATE_MAP: OnceCell<HashMap<&'static str, ProductState>> = OnceCell::new();

fn state_map() -> &'static HashMap<&'static str, ProductState> {
    STATE_MAP.get_or_init(|| {
        HashMap::from([
            // English
            ("available", ProductState::Available),
            ("in stock", ProductState::Available),
            ("add to cart", ProductState::Available),
            ("add to basket", ProductState::Available),
            ("buy now", ProductState::Available),
            ("listed", ProductState::Listed),
            ("reserved", ProductState::Reserved),
            ("on hold", ProductState::Reserved),
            ("sold", ProductState::Sold),
            ("sold out", ProductState::Sold),
            ("out of stock", ProductState::Sold),
            ("removed", ProductState::Removed),
            ("deleted", ProductState::Removed),
            ("unavailable", ProductState::Removed),
            // German
            ("in den warenkorb", ProductState::Available),
            ("verfügbar", ProductState::Available),
            ("auf lager", ProductState::Available),
            ("gelistet", ProductState::Listed),
            ("reserviert", ProductState::Reserved),
            ("verkauft", ProductState::Sold),
            ("ausverkauft", ProductState::Sold),
            ("gelöscht", ProductState::Removed),
            ("entfernt", ProductState::Removed),
            // French
            ("disponible", ProductState::Available),
            ("en stock", ProductState::Available),
            ("listé", ProductState::Listed),
            ("liste", ProductState::Listed),
            ("réservé", ProductState::Reserved),
            ("reserve", ProductState::Reserved),
            ("vendu", ProductState::Sold),
            ("épuisé", ProductState::Sold),
            ("supprimé", ProductState::Removed),
            // Spanish
            ("listado", ProductState::Listed),
            ("reservado", ProductState::Reserved),
            ("vendido", ProductState::Sold),
            ("eliminado", ProductState::Removed),
            // Italian
            ("disponibile", ProductState::Available),
            ("inserito", ProductState::Listed),
            ("riservato", ProductState::Reserved),
            ("venduto", ProductState::Sold),
            ("rimosso", ProductState::Removed),
        ])
    })
}

// ---------------------------------------------------------------------------
// Regex-based fallback patterns
// ---------------------------------------------------------------------------

/// Each entry pairs a compiled [`Regex`] with the [`ProductState`] it implies.
///
/// Patterns are tried after the exact-match lookup fails. Declaration order
/// does **not** matter for correctness: every pattern encodes the quantity
/// sign directly in the regex, so there is no ambiguity between groups.
///
/// Design notes
/// ────────────
/// • All patterns are matched against the *trimmed, lower-cased* input so the
///   regexes themselves can stay ASCII-only where possible.
/// • `POS` (`[1-9][0-9]*`) matches any strictly-positive integer and maps to
///   `Available`.  `\b0\b` matches the literal zero and maps to `Sold`.
///   The two groups are therefore mutually exclusive by construction.
/// • Redundant specificity variants (e.g. separate patterns for "nur noch N"
///   and "noch N" and plain "N") are collapsed into one pattern with optional
///   leading words, which was only necessary before to preserve ordering.
/// • Word-boundary assertions (`\b`) prevent spurious matches inside longer
///   words (e.g. "unavailable" must not match "available").
/// • Coverage across English, German, French, Spanish and Italian, e.g.:
///     - "1 available"              → Available
///     - "only 4 remaining"         → Available
///     - "nur noch 7 verfügbar"     → Available
///     - "3 en stock"               → Available
///     - "solo 2 disponibili"       → Available
///     - "0 available"              → Sold
///     - "0 verfügbar"              → Sold
static STATE_PATTERNS: OnceCell<Vec<(Regex, ProductState)>> = OnceCell::new();

fn state_patterns() -> &'static Vec<(Regex, ProductState)> {
    STATE_PATTERNS.get_or_init(|| {
        // Helper closure — panics at startup on an invalid pattern, which is
        // intentional: a broken regex is a programming error, not runtime data.
        let re = |pat: &str| Regex::new(pat).expect("invalid state regex pattern");

        // Strictly-positive integer: excludes zero so Available/Sold patterns
        // are mutually exclusive without depending on declaration order.
        const POS: &str = r"[1-9][0-9]*";

        vec![
            // ── Available — positive quantity ─────────────────────────────
            // English: "1 available", "3 available now"
            (
                re(&format!(r"{POS}\s+available\b")),
                ProductState::Available,
            ),
            // English: "only 4 remaining", "just 1 remaining", "2 remaining"
            (
                re(&format!(r"\b(only\s+|just\s+)?{POS}\s+remaining\b")),
                ProductState::Available,
            ),
            // English: "only 2 left", "1 left", "1 left in stock"
            (
                re(&format!(r"\b(only\s+)?{POS}\s+left\b")),
                ProductState::Available,
            ),
            // English: "5 in stock"
            (
                re(&format!(r"{POS}\s+in\s+stock\b")),
                ProductState::Available,
            ),
            // English: "hurry, only 3 left!", "hurry! 2 remaining"
            (re(&format!(r"\bhurry\b.*{POS}")), ProductState::Available),
            // German: "x vorrätig"
            (re(&format!(r"{POS}\s+vorrätig\b")), ProductState::Available),
            // German: "nur noch 7 verfügbar", "noch 2 verfügbar", "3 verfügbar"
            (
                re(&format!(r"(\bnur\s+)?(\bnoch\s+)?{POS}\s+verfügbar\b")),
                ProductState::Available,
            ),
            // German: "nur noch 3 auf lager", "noch 1 auf lager", "4 auf lager"
            (
                re(&format!(r"(\bnur\s+)?(\bnoch\s+)?{POS}\s+auf\s+lager\b")),
                ProductState::Available,
            ),
            // German: "nur noch 2 stück", "noch 5 stück", "3 stück verfügbar"
            (
                re(&format!(r"(\bnur\s+)?(\bnoch\s+)?{POS}\s+stück\b")),
                ProductState::Available,
            ),
            // French: "3 en stock", "plus que 2 en stock"
            (
                re(&format!(r"(\bplus\s+que\s+)?{POS}\s+en\s+stock\b")),
                ProductState::Available,
            ),
            // French: "2 disponible(s)", "1 disponible"
            (
                re(&format!(r"{POS}\s+disponibles?\b")),
                ProductState::Available,
            ),
            // French: "il reste 3", "il ne reste que 1"
            (
                re(&format!(r"\bil\s+(ne\s+)?reste\s+(que\s+)?{POS}\b")),
                ProductState::Available,
            ),
            // Spanish: "solo 2 disponibles", "3 disponibles", "quedan 3"
            (
                re(&format!(r"(\bsolo\s+)?{POS}\s+disponibles?\b")),
                ProductState::Available,
            ),
            (re(&format!(r"\bquedan\s+{POS}\b")), ProductState::Available),
            // Italian: "solo 2 disponibili", "4 disponibili", "rimangono 3"
            (
                re(&format!(r"(\bsolo\s+)?{POS}\s+disponibili\b")),
                ProductState::Available,
            ),
            (
                re(&format!(r"\brimangono\s+{POS}\b")),
                ProductState::Available,
            ),
            // ── Sold — zero quantity ──────────────────────────────────────
            (re(r"\b0\s+available\b"), ProductState::Sold),
            (re(r"\b0\s+remaining\b"), ProductState::Sold),
            (re(r"\b0\s+left\b"), ProductState::Sold),
            (re(r"\b0\s+in\s+stock\b"), ProductState::Sold),
            (re(r"\b0\s+verfügbar\b"), ProductState::Sold),
            (re(r"\b0\s+auf\s+lager\b"), ProductState::Sold),
            (re(r"\b0\s+stück\b"), ProductState::Sold),
            (re(r"\b0\s+en\s+stock\b"), ProductState::Sold),
            (re(r"\b0\s+disponibles?\b"), ProductState::Sold),
            (re(r"\b0\s+disponibili\b"), ProductState::Sold),
        ]
    })
}

// ---------------------------------------------------------------------------
// Public helper
// ---------------------------------------------------------------------------

/// Normalises a raw scraper state string into a [`ProductState`].
///
/// Resolution order:
/// 1. Exact match in the static [`STATE_MAP`] (fastest path).
/// 2. First matching pattern in [`STATE_PATTERNS`] (quantity-style strings).
/// 3. [`ProductState::Unknown`] — an LLM lookup will replace this fallback in
///    the future.
pub(super) fn normalize_state(raw: &str) -> ProductState {
    let key = raw.trim().to_lowercase();

    if let Some(&state) = state_map().get(key.as_str()) {
        return state;
    }

    for (pattern, state) in state_patterns() {
        if pattern.is_match(&key) {
            return *state;
        }
    }

    ProductState::Unknown
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use common::product_state::domain::ProductState;

    use super::normalize_state;

    #[rstest]
    // English — available
    #[case("available", ProductState::Available)]
    #[case("Available", ProductState::Available)]
    #[case("AVAILABLE", ProductState::Available)]
    #[case("in stock", ProductState::Available)]
    #[case("add to cart", ProductState::Available)]
    #[case("buy now", ProductState::Available)]
    // English — other
    #[case("listed", ProductState::Listed)]
    #[case("reserved", ProductState::Reserved)]
    #[case("on hold", ProductState::Reserved)]
    #[case("sold", ProductState::Sold)]
    #[case("sold out", ProductState::Sold)]
    #[case("out of stock", ProductState::Sold)]
    #[case("removed", ProductState::Removed)]
    #[case("deleted", ProductState::Removed)]
    #[case("unavailable", ProductState::Removed)]
    // German
    #[case("verfügbar", ProductState::Available)]
    #[case("auf lager", ProductState::Available)]
    #[case("gelistet", ProductState::Listed)]
    #[case("reserviert", ProductState::Reserved)]
    #[case("verkauft", ProductState::Sold)]
    #[case("ausverkauft", ProductState::Sold)]
    #[case("gelöscht", ProductState::Removed)]
    #[case("entfernt", ProductState::Removed)]
    // French
    #[case("disponible", ProductState::Available)]
    #[case("en stock", ProductState::Available)]
    #[case("listé", ProductState::Listed)]
    #[case("liste", ProductState::Listed)]
    #[case("réservé", ProductState::Reserved)]
    #[case("reserve", ProductState::Reserved)]
    #[case("vendu", ProductState::Sold)]
    #[case("épuisé", ProductState::Sold)]
    #[case("supprimé", ProductState::Removed)]
    // Spanish
    #[case("listado", ProductState::Listed)]
    #[case("reservado", ProductState::Reserved)]
    #[case("vendido", ProductState::Sold)]
    #[case("eliminado", ProductState::Removed)]
    // Italian
    #[case("disponibile", ProductState::Available)]
    #[case("inserito", ProductState::Listed)]
    #[case("riservato", ProductState::Reserved)]
    #[case("venduto", ProductState::Sold)]
    #[case("rimosso", ProductState::Removed)]
    // Case-insensitive
    #[case("Verfügbar", ProductState::Available)]
    #[case("VERKAUFT", ProductState::Sold)]
    #[case("Vendu", ProductState::Sold)]
    // Unknown / unrecognised
    #[case("some random text", ProductState::Unknown)]
    #[case("", ProductState::Unknown)]
    #[case("   ", ProductState::Unknown)]
    fn should_normalize_state_when_raw_value_provided(
        #[case] raw: &str,
        #[case] expected: ProductState,
    ) {
        assert_eq!(normalize_state(raw), expected);
    }

    #[test]
    fn should_trim_whitespace_when_normalizing_state() {
        assert_eq!(normalize_state("  available  "), ProductState::Available);
    }

    // ── Regex fallback — quantity-style "N available" strings ────────────

    #[rstest]
    // English — positive quantity
    #[case("1 available", ProductState::Available)]
    #[case("3 available", ProductState::Available)]
    #[case("12 available now", ProductState::Available)]
    #[case("only 4 remaining", ProductState::Available)]
    #[case("just 1 remaining", ProductState::Available)]
    #[case("2 remaining", ProductState::Available)]
    #[case("only 2 left", ProductState::Available)]
    #[case("1 left", ProductState::Available)]
    #[case("5 in stock", ProductState::Available)]
    #[case("hurry, only 3 left!", ProductState::Available)]
    #[case("hurry! 2 remaining", ProductState::Available)]
    // English — zero quantity → sold
    #[case("0 available", ProductState::Sold)]
    #[case("0 in stock", ProductState::Sold)]
    #[case("0 left", ProductState::Sold)]
    // German — positive quantity
    #[case("nur noch 7 verfügbar", ProductState::Available)]
    #[case("Nur noch 7 verfügbar", ProductState::Available)]
    #[case("NUR NOCH 7 VERFÜGBAR", ProductState::Available)]
    #[case("noch 2 verfügbar", ProductState::Available)]
    #[case("3 verfügbar", ProductState::Available)]
    #[case("nur noch 3 auf lager", ProductState::Available)]
    #[case("noch 1 auf lager", ProductState::Available)]
    #[case("4 auf lager", ProductState::Available)]
    #[case("nur noch 2 stück", ProductState::Available)]
    #[case("noch 5 stück verfügbar", ProductState::Available)]
    // German — zero quantity → sold
    #[case("0 verfügbar", ProductState::Sold)]
    // French — positive quantity
    #[case("3 en stock", ProductState::Available)]
    #[case("plus que 2 en stock", ProductState::Available)]
    #[case("2 disponibles", ProductState::Available)]
    #[case("1 disponible", ProductState::Available)]
    #[case("il reste 3", ProductState::Available)]
    #[case("il ne reste que 1", ProductState::Available)]
    // French — zero quantity → sold
    #[case("0 en stock", ProductState::Sold)]
    #[case("0 disponibles", ProductState::Sold)]
    // Spanish — positive quantity
    #[case("solo 2 disponibles", ProductState::Available)]
    #[case("3 disponibles", ProductState::Available)]
    #[case("quedan 3", ProductState::Available)]
    // Spanish — zero quantity → sold
    #[case("0 disponibles", ProductState::Sold)]
    // Italian — positive quantity
    #[case("solo 2 disponibili", ProductState::Available)]
    #[case("4 disponibili", ProductState::Available)]
    #[case("rimangono 3", ProductState::Available)]
    // Italian — zero quantity → sold
    #[case("0 disponibili", ProductState::Sold)]
    fn should_normalize_state_via_regex_when_quantity_style_string_provided(
        #[case] raw: &str,
        #[case] expected: ProductState,
    ) {
        assert_eq!(normalize_state(raw), expected);
    }
}
