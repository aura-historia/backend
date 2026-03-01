use common::product_state::domain::ProductState;
use once_cell::sync::OnceCell;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Static lookup table
// ---------------------------------------------------------------------------

/// Maps trimmed, lower-cased raw state strings to a [`ProductState`].
///
/// This is the OnceCell-backed "database" that will later be replaced by a
/// real database call. If a value is not found here the service will fall back
/// to `ProductState::Unknown` and (in the future) ask an LLM for the mapping.
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
// Public helper
// ---------------------------------------------------------------------------

/// Looks up the trimmed, lower-cased `raw` string in the static state map.
///
/// Returns `ProductState::Unknown` when the value is not recognised — an LLM
/// lookup and subsequent caching will replace this fallback in the future.
pub(super) fn normalize_state(raw: &str) -> ProductState {
    let key = raw.trim().to_lowercase();
    state_map()
        .get(key.as_str())
        .copied()
        .unwrap_or(ProductState::Unknown)
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
}
