use super::error::NormalizationError;
use common::{
    currency::domain::{Currency, HasMinorUnitExponent},
    price::domain::{MonetaryAmount, Price},
};
use once_cell::sync::OnceCell;
use regex::Regex;
use tracing::debug;
use url::Url;

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

/// Internal error for price parsing — carries no field context yet.
/// Callers map this to the appropriate [`NormalizationError`] variant.
#[derive(Debug, PartialEq)]
pub(super) enum PriceError {
    /// No recognised currency symbol or ISO code was found in the string.
    UnknownCurrency,
    /// A currency was detected but the numeric amount could not be parsed.
    ParseFailure,
}

// ---------------------------------------------------------------------------
// Currency detection
// ---------------------------------------------------------------------------

/// Detects the currency from a raw price string.
///
/// Returns `None` if no recognized currency symbol or ISO code is present.
/// Multi-character symbols (`NZD`, `AUD`, `CAD`) are checked before the plain
/// `$` to avoid false matches.
pub(super) fn detect_currency(raw: &str) -> Option<Currency> {
    if raw.contains("NZD") || raw.contains("NZ$") {
        Some(Currency::Nzd)
    } else if raw.contains("AUD") || raw.contains("A$") {
        Some(Currency::Aud)
    } else if raw.contains("CAD") || raw.contains("C$") {
        Some(Currency::Cad)
    } else if raw.contains("USD") || raw.contains('$') {
        Some(Currency::Usd)
    } else if raw.contains("GBP") || raw.contains('£') {
        Some(Currency::Gbp)
    } else if raw.contains("EUR") || raw.contains('€') {
        Some(Currency::Eur)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Price parsing
// ---------------------------------------------------------------------------

/// Strips currency symbols / codes and converts the remaining string to a
/// [`MonetaryAmount`] + [`Currency`] pair.
///
/// Handles formats such as:
///   - `"1.234,56 €"`   (European: dot-thousands, comma-decimal)
///   - `"1,234.56 USD"` (Anglo: comma-thousands, dot-decimal)
///   - `"£1 234.56"`    (space-thousands)
///   - `"1234.5"`       (single decimal digit)
///   - `"1'234.56"`     (apostrophe-thousands)
///
/// If no currency marker is found in `raw` the optional `fallback_currency` is
/// used (e.g. inferred from the shop's domain TLD).  If neither is present
/// [`PriceError::UnknownCurrency`] is returned.
pub(super) fn parse_price(
    raw: &str,
    fallback_currency: Option<Currency>,
) -> Result<(MonetaryAmount, Currency), PriceError> {
    let currency = detect_currency(raw)
        .or(fallback_currency)
        .ok_or(PriceError::UnknownCurrency)?;

    // Remove known currency symbols / codes so they don't confuse the number
    // parser.
    static CURRENCY_PATTERN: OnceCell<Regex> = OnceCell::new();
    let re = CURRENCY_PATTERN
        .get_or_init(|| Regex::new(r"(?i)(EUR|USD|GBP|AUD|CAD|NZD|NZ\$|A\$|C\$|\$|£|€)").unwrap());
    let stripped = re.replace_all(raw, "");

    // Remove whitespace and apostrophes used as thousands separators.
    let stripped = stripped.replace([' ', '\u{00a0}', '\'', '_'], "");

    // Determine whether comma or dot is the decimal separator, then split.
    let (integer_part, fraction_part): (&str, &str) =
        if let Some((left, right)) = split_decimal(&stripped) {
            (left, right)
        } else {
            (stripped.as_str(), "0")
        };

    // Strip remaining thousands-separator characters from each part.
    let integer_clean: String = integer_part
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    let fraction_clean: String = fraction_part
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();

    if integer_clean.is_empty() && fraction_clean == "0" {
        return Err(PriceError::ParseFailure);
    }

    let exponent = minor_unit_exponent(&currency);
    let fraction_normalised = normalise_fraction(&fraction_clean, exponent);

    let amount_str = format!("{}{}", integer_clean, fraction_normalised);
    let amount: u64 = amount_str.parse().map_err(|_| PriceError::ParseFailure)?;

    Ok((MonetaryAmount::from(amount), currency))
}

// ---------------------------------------------------------------------------
// Public field-level helper
// ---------------------------------------------------------------------------

/// Parses an optional raw price string into an optional [`Price`].
///
/// - `None` input → `Ok(None)`
/// - blank string → `Ok(None)`
/// - unknown currency (and no `fallback_currency`) → `Err(make_currency_err(raw))`
/// - unparseable amount → `Err(make_parse_err(raw))`
///
/// `fallback_currency` is used when the raw string contains no currency symbol
/// or ISO code — typically the `default_currency` stored in the shop's
/// [`ProductCssSelectorSchema`] and set by the LLM during schema creation.
pub(super) fn normalize_price_field(
    raw: Option<String>,
    field_name: &'static str,
    context_url: &Url,
    fallback_currency: Option<Currency>,
    make_currency_err: impl Fn(String) -> NormalizationError,
    make_parse_err: impl Fn(String) -> NormalizationError,
) -> Result<Option<Price>, NormalizationError> {
    let Some(s) = raw else { return Ok(None) };

    let trimmed = s.trim().to_owned();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if is_price_on_request_marker(&trimmed) {
        debug!(
            url = %context_url,
            field = field_name,
            raw_price = %trimmed,
            "Price text indicates 'price on request'; defaulting normalized price to None"
        );
        return Ok(None);
    }

    match parse_price(&trimmed, fallback_currency) {
        Ok((amount, currency)) => Ok(Some(Price::new(amount, currency))),
        Err(PriceError::UnknownCurrency) => Err(make_currency_err(trimmed)),
        Err(PriceError::ParseFailure) => Err(make_parse_err(trimmed)),
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Returns the number of minor-unit digits for the given currency (always 2
/// for the currencies we support).
fn minor_unit_exponent(currency: &Currency) -> u32 {
    currency.minor_unit_exponent().0 as u32
}

/// Pads with trailing zeros or truncates to exactly `exponent` digits.
fn normalise_fraction(frac: &str, exponent: u32) -> String {
    let exp = exponent as usize;
    if frac.len() >= exp {
        frac[..exp].to_owned()
    } else {
        format!("{:0<width$}", frac, width = exp)
    }
}

/// Splits a cleaned number string at the decimal separator.
///
/// The *later* of the last `.` and last `,` is treated as the decimal
/// separator; the earlier one (if present) is the thousands separator.
fn split_decimal(s: &str) -> Option<(&str, &str)> {
    let last_dot = s.rfind('.');
    let last_comma = s.rfind(',');

    match (last_dot, last_comma) {
        (None, None) => None,
        (Some(d), None) => Some((&s[..d], &s[d + 1..])),
        (None, Some(c)) => Some((&s[..c], &s[c + 1..])),
        (Some(d), Some(c)) => {
            if d > c {
                Some((&s[..d], &s[d + 1..]))
            } else {
                Some((&s[..c], &s[c + 1..]))
            }
        }
    }
}

fn is_price_on_request_marker(raw: &str) -> bool {
    // Keywords that, when present anywhere in the price string, indicate that
    // the seller intentionally has not set a price and it must be requested.
    // Covers: EN, DE, FR, IT, ES, PT, NL, PL, RU, ZH, JA, AR
    const KEYWORDS: &[&str] = &[
        // English
        "request", // "on request", "price on request", "price upon request"
        "enquire", // "please enquire"
        "inquire", // "please inquire"
        "contact us",
        "call for price",
        "ask for price",
        // German
        "anfrage", // "auf Anfrage", "Preis auf Anfrage"
        // French
        "demande", // "sur demande", "prix sur demande"
        // Italian
        "richiesta", // "su richiesta", "prezzo su richiesta"
        // Spanish
        "consultar", // "precio a consultar"
        "bajo pedido",
        // Portuguese
        "consulte",     // "consulte-nos"
        "sob consulta", // "preço sob consulta"
        // Dutch
        "aanvraag", // "op aanvraag", "prijs op aanvraag"
        // Polish
        "zapytanie", // "na zapytanie", "cena na zapytanie"
        // Russian
        "по запросу", // "цена по запросу"
        // Chinese
        "询价",
        "面议",
        // Japanese
        "お問い合わせ", // "価格はお問い合わせ"
        // Arabic
        "بالتفاوض",
        "عند الطلب",
    ];

    let lower = raw.to_lowercase();
    KEYWORDS.iter().any(|kw| lower.contains(kw))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use common::currency::domain::Currency;

    use super::{
        PriceError, detect_currency, is_price_on_request_marker, normalise_fraction, parse_price,
        split_decimal,
    };

    // -----------------------------------------------------------------------
    // detect_currency
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("$100", Some(Currency::Usd))]
    #[case("USD 100", Some(Currency::Usd))]
    #[case("£100", Some(Currency::Gbp))]
    #[case("GBP 100", Some(Currency::Gbp))]
    #[case("€ 100", Some(Currency::Eur))]
    #[case("EUR 100", Some(Currency::Eur))]
    #[case("A$100", Some(Currency::Aud))]
    #[case("AUD 100", Some(Currency::Aud))]
    #[case("C$100", Some(Currency::Cad))]
    #[case("CAD 100", Some(Currency::Cad))]
    #[case("NZ$100", Some(Currency::Nzd))]
    #[case("NZD 100", Some(Currency::Nzd))]
    #[case("100", None)]
    #[case("CHF 100", None)]
    #[case("", None)]
    fn should_detect_currency_when_symbol_or_code_present(
        #[case] raw: &str,
        #[case] expected: Option<Currency>,
    ) {
        assert_eq!(detect_currency(raw), expected);
    }

    #[test]
    fn should_prefer_nzd_over_plain_dollar_when_nz_prefix_present() {
        assert_eq!(detect_currency("NZ$100"), Some(Currency::Nzd));
    }

    #[test]
    fn should_prefer_aud_over_plain_dollar_when_a_prefix_present() {
        assert_eq!(detect_currency("A$100"), Some(Currency::Aud));
    }

    #[test]
    fn should_prefer_cad_over_plain_dollar_when_c_prefix_present() {
        assert_eq!(detect_currency("C$100"), Some(Currency::Cad));
    }

    // -----------------------------------------------------------------------
    // parse_price — successful cases
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("1.234,56 €", 123456, Currency::Eur)]
    #[case("1,234.56 USD", 123456, Currency::Usd)]
    #[case("£1 234.56", 123456, Currency::Gbp)]
    #[case("100 EUR", 10000, Currency::Eur)]
    #[case("€ 50,00", 5000, Currency::Eur)]
    #[case("$9.99", 999, Currency::Usd)]
    #[case("A$12.50", 1250, Currency::Aud)]
    #[case("C$99.99", 9999, Currency::Cad)]
    #[case("NZ$25.00", 2500, Currency::Nzd)]
    #[case("GBP 1234", 123400, Currency::Gbp)]
    #[case("0.50 USD", 50, Currency::Usd)]
    #[case("1.5 EUR", 150, Currency::Eur)]
    #[case("1'234.56 USD", 123456, Currency::Usd)]
    #[case("£ 0.01", 1, Currency::Gbp)]
    fn should_parse_price_when_valid_string_provided(
        #[case] raw: &str,
        #[case] expected_amount: u64,
        #[case] expected_currency: Currency,
    ) {
        let (amount, currency) = parse_price(raw, None).unwrap();
        assert_eq!(*amount, expected_amount, "amount mismatch for '{}'", raw);
        assert_eq!(
            currency, expected_currency,
            "currency mismatch for '{}'",
            raw
        );
    }

    // -----------------------------------------------------------------------
    // parse_price — error cases
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("no numbers here USD")]
    #[case("€")]
    fn should_return_parse_failure_when_price_has_currency_but_no_number(#[case] raw: &str) {
        assert!(
            matches!(parse_price(raw, None), Err(PriceError::ParseFailure)),
            "expected ParseFailure for '{}'",
            raw
        );
    }

    #[rstest]
    #[case("")]
    #[case("   ")]
    #[case("no numbers here")]
    #[case("1234.56 CHF")]
    fn should_return_unknown_currency_when_no_currency_symbol_or_known_code(#[case] raw: &str) {
        assert!(
            matches!(parse_price(raw, None), Err(PriceError::UnknownCurrency)),
            "expected UnknownCurrency for '{}'",
            raw
        );
    }

    // -----------------------------------------------------------------------
    // parse_price — fallback currency
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("18,00", Currency::Eur, 1800u64)]
    #[case("1590", Currency::Eur, 159000u64)]
    #[case("1590", Currency::Gbp, 159000u64)]
    #[case("1.234,56", Currency::Eur, 123456u64)]
    fn should_parse_bare_price_when_fallback_currency_provided(
        #[case] raw: &str,
        #[case] fallback: Currency,
        #[case] expected_amount: u64,
    ) {
        let (amount, currency) = parse_price(raw, Some(fallback)).unwrap();
        assert_eq!(*amount, expected_amount, "amount mismatch for '{}'", raw);
        assert_eq!(currency, fallback, "currency mismatch for '{}'", raw);
    }

    // -----------------------------------------------------------------------
    // split_decimal
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("1234", None)]
    #[case("1234.56", Some(("1234", "56")))]
    #[case("1234,56", Some(("1234", "56")))]
    // dot is later → dot is decimal separator
    #[case("1.234,56", Some(("1.234", "56")))]
    // comma is later → comma is decimal separator
    #[case("1,234.56", Some(("1,234", "56")))]
    fn should_split_decimal_correctly_for_various_formats(
        #[case] input: &str,
        #[case] expected: Option<(&str, &str)>,
    ) {
        assert_eq!(split_decimal(input), expected);
    }

    // -----------------------------------------------------------------------
    // normalise_fraction
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("56", 2, "56")]
    #[case("5", 2, "50")]
    #[case("", 2, "00")]
    #[case("123", 2, "12")]
    #[case("0", 2, "00")]
    fn should_normalise_fraction_to_correct_width(
        #[case] frac: &str,
        #[case] exponent: u32,
        #[case] expected: &str,
    ) {
        assert_eq!(normalise_fraction(frac, exponent), expected);
    }

    // -----------------------------------------------------------------------
    // is_price_on_request_marker
    // -----------------------------------------------------------------------

    #[rstest]
    // English
    #[case("Price on Request", true)]
    #[case("price available on request", true)]
    #[case("Please enquire", true)]
    #[case("Call for price", true)]
    // German
    #[case("Preis auf Anfrage", true)]
    #[case("auf Anfrage", true)]
    // French
    #[case("Prix sur demande", true)]
    #[case("sur demande", true)]
    // Italian
    #[case("Prezzo su richiesta", true)]
    #[case("Su Richiesta", true)]
    // Spanish
    #[case("Precio a consultar", true)]
    #[case("Consultar", true)]
    // Portuguese
    #[case("Preço sob consulta", true)]
    // Dutch
    #[case("Prijs op aanvraag", true)]
    // Polish
    #[case("Cena na zapytanie", true)]
    // Russian
    #[case("Цена по запросу", true)]
    // Chinese
    #[case("询价", true)]
    #[case("面议", true)]
    // Japanese
    #[case("価格はお問い合わせ", true)]
    // Arabic
    #[case("عند الطلب", true)]
    // Not markers
    #[case("$1200", false)]
    #[case("EUR 45", false)]
    #[case("1.500,00 €", false)]
    fn should_detect_price_on_request_markers(#[case] raw: &str, #[case] expected: bool) {
        assert_eq!(is_price_on_request_marker(raw), expected);
    }
}
