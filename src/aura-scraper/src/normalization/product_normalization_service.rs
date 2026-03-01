use std::collections::HashMap;

use common::{
    currency::domain::Currency,
    language::domain::Language,
    localized::Localized,
    price::domain::{MonetaryAmount, Price},
    product_state::domain::ProductState,
    shops_product_id::ShopsProductId,
};
use once_cell::sync::OnceCell;
use product::core::{
    description::Description, product_image::ProductImage, prohibited_content::ProhibitedContent,
    title::Title,
};
use regex::Regex;
use time::{OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description};
use url::Url;

use crate::{
    css_selector::product_schema::RawExtractedProduct, normalization::product::NormalizedProduct,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum NormalizationError {
    #[error("failed to normalize `shops_product_id`: value is empty after trimming")]
    ShopsProductIdEmpty,

    #[error("failed to normalize `title`: value is empty after trimming")]
    TitleEmpty,

    #[error("failed to normalize `title`: could not detect language of '{text}'")]
    TitleUnknownLanguage { text: String },

    #[error("failed to normalize `description`: could not detect language of '{text}'")]
    DescriptionUnknownLanguage { text: String },

    #[error("failed to normalize `price`: could not detect currency in '{raw}'")]
    PriceUnknownCurrency { raw: String },

    #[error("failed to normalize `price`: could not parse '{raw}' as a monetary amount")]
    PriceParseError { raw: String },

    #[error("failed to normalize `price_estimate_min`: could not detect currency in '{raw}'")]
    PriceEstimateMinUnknownCurrency { raw: String },

    #[error(
        "failed to normalize `price_estimate_min`: could not parse '{raw}' as a monetary amount"
    )]
    PriceEstimateMinParseError { raw: String },

    #[error("failed to normalize `price_estimate_max`: could not detect currency in '{raw}'")]
    PriceEstimateMaxUnknownCurrency { raw: String },

    #[error(
        "failed to normalize `price_estimate_max`: could not parse '{raw}' as a monetary amount"
    )]
    PriceEstimateMaxParseError { raw: String },

    #[error("failed to normalize `images`: invalid URL '{raw}': {source}")]
    InvalidImageUrl {
        raw: String,
        #[source]
        source: url::ParseError,
    },

    #[error("failed to normalize `auction_start`: could not parse '{raw}' as a date/time")]
    AuctionStartParseError { raw: String },

    #[error("failed to normalize `auction_end`: could not parse '{raw}' as a date/time")]
    AuctionEndParseError { raw: String },
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductNormalizationService {
    fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
    ) -> Result<NormalizedProduct, NormalizationError>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ProductNormalizationServiceImpl;

impl ProductNormalizationServiceImpl {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProductNormalizationServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Static lookup tables (OnceCell-backed "database" – LLM fallback later)
// ---------------------------------------------------------------------------

/// Maps trimmed raw state strings (lower-cased) to a `ProductState`.
static STATE_MAP: OnceCell<HashMap<&'static str, ProductState>> = OnceCell::new();

fn state_map() -> &'static HashMap<&'static str, ProductState> {
    STATE_MAP.get_or_init(|| {
        HashMap::from([
            // English
            ("available", ProductState::Available),
            ("in stock", ProductState::Available),
            ("add to cart", ProductState::Available),
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
            ("disponible", ProductState::Available),
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
// Field-level helpers
// ---------------------------------------------------------------------------

fn normalize_shops_product_id(raw: &str) -> Result<ShopsProductId, NormalizationError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NormalizationError::ShopsProductIdEmpty);
    }
    Ok(ShopsProductId::from(trimmed))
}

fn normalize_title(raw: &str) -> Result<Title, NormalizationError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NormalizationError::TitleEmpty);
    }
    Ok(Title::from(trimmed))
}

fn normalize_description(
    fragments: Vec<String>,
) -> Result<Option<Localized<Language, Description>>, NormalizationError> {
    // Drop blank fragments, trim each one, join as paragraphs.
    let cleaned: Vec<String> = fragments
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    if cleaned.is_empty() {
        return Ok(None);
    }

    let joined = cleaned.join("\n\n");
    let description = Description::from(joined.as_str());
    let language = detect_language(description.as_ref()).ok_or_else(|| {
        NormalizationError::DescriptionUnknownLanguage {
            text: description.as_ref().chars().take(100).collect(),
        }
    })?;

    Ok(Some(Localized::new(language, description)))
}

/// Detect the language of a text snippet. Returns `None` if the language
/// cannot be identified as one of the supported languages.
fn detect_language(text: &str) -> Option<Language> {
    whatlang::detect_lang(text).and_then(|lang| match lang {
        whatlang::Lang::Deu => Some(Language::De),
        whatlang::Lang::Eng => Some(Language::En),
        whatlang::Lang::Fra => Some(Language::Fr),
        whatlang::Lang::Spa => Some(Language::Es),
        whatlang::Lang::Ita => Some(Language::It),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Price parsing
// ---------------------------------------------------------------------------

// Internal error type for price parsing, before field-level context is added.
#[derive(Debug)]
enum PriceError {
    UnknownCurrency,
    ParseFailure,
}

/// Strips non-numeric characters (except `.` and `,`) and converts the string
/// to a `Price`.
///
/// Handles formats such as:
///   - `"1.234,56 €"`  (European: dot-thousands, comma-decimal)
///   - `"1,234.56 USD"` (Anglo: comma-thousands, dot-decimal)
///   - `"£1 234.56"`   (space-thousands)
///   - `"1234.5"`      (single decimal digit)
///   - `"1'234.56"`    (apostrophe-thousands)
fn parse_price(raw: &str) -> Result<(MonetaryAmount, Currency), PriceError> {
    let currency = detect_currency(raw).ok_or(PriceError::UnknownCurrency)?;

    // Remove known currency symbols / codes first so they don't confuse the
    // number parser, then strip any remaining non-numeric / non-separator chars.
    static CURRENCY_PATTERN: OnceCell<Regex> = OnceCell::new();
    let re = CURRENCY_PATTERN
        .get_or_init(|| Regex::new(r"(?i)(EUR|USD|GBP|AUD|CAD|NZD|NZ\$|A\$|C\$|\$|£|€)").unwrap());
    let stripped = re.replace_all(raw, "");

    // Remove whitespace, apostrophes used as thousands-separators
    let stripped = stripped.replace([' ', '\u{00a0}', '\'', '_'], "");

    // Now we have something like "1.234,56", "1,234.56", "1234", "1234.56"
    // Determine whether comma or dot is the decimal separator.
    let (integer_part, fraction_part): (&str, &str) =
        if let Some((left, right)) = split_decimal(&stripped) {
            (left, right)
        } else {
            (stripped.as_str(), "0")
        };

    // Remove thousands separators from integer part.
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

    let exponent = currency.minor_unit_exponent_value();

    // Pad / truncate the fraction to exactly `exponent` digits.
    let fraction_normalised = normalise_fraction(&fraction_clean, exponent);

    let amount_str = format!("{}{}", integer_clean, fraction_normalised);
    let amount: u64 = amount_str.parse().map_err(|_| PriceError::ParseFailure)?;

    Ok((MonetaryAmount::from(amount), currency))
}

/// Returns how many decimal digits the currency uses (always 2 for our set).
trait MinorUnitExponentValue {
    fn minor_unit_exponent_value(&self) -> u32;
}

impl MinorUnitExponentValue for Currency {
    fn minor_unit_exponent_value(&self) -> u32 {
        use common::currency::domain::HasMinorUnitExponent;
        self.minor_unit_exponent().0 as u32
    }
}

/// Pad with trailing zeros or truncate to the required number of digits.
fn normalise_fraction(frac: &str, exponent: u32) -> String {
    let exp = exponent as usize;
    if frac.len() >= exp {
        frac[..exp].to_owned()
    } else {
        format!("{:0<width$}", frac, width = exp)
    }
}

/// Splits a number string at the decimal separator.  We decide by looking at
/// the last occurrence of `.` or `,`; the other character (if present) must be
/// a thousands separator.
fn split_decimal(s: &str) -> Option<(&str, &str)> {
    let last_dot = s.rfind('.');
    let last_comma = s.rfind(',');

    match (last_dot, last_comma) {
        (None, None) => None,
        (Some(d), None) => Some((&s[..d], &s[d + 1..])),
        (None, Some(c)) => Some((&s[..c], &s[c + 1..])),
        (Some(d), Some(c)) => {
            // The *later* of the two is the decimal separator.
            if d > c {
                Some((&s[..d], &s[d + 1..]))
            } else {
                Some((&s[..c], &s[c + 1..]))
            }
        }
    }
}

/// Detects the currency from a raw price string.
///
/// Returns `None` if no recognised currency symbol or code is present.
fn detect_currency(raw: &str) -> Option<Currency> {
    // Check for NZD/AUD/CAD before plain `$` to avoid false matches.
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

fn normalize_price_field(
    raw: Option<String>,
    make_currency_err: impl Fn(String) -> NormalizationError,
    make_parse_err: impl Fn(String) -> NormalizationError,
) -> Result<Option<Price>, NormalizationError> {
    match raw {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim().to_owned();
            if trimmed.is_empty() {
                return Ok(None);
            }
            match parse_price(&trimmed) {
                Ok((amount, currency)) => Ok(Some(Price::new(amount, currency))),
                Err(PriceError::UnknownCurrency) => Err(make_currency_err(trimmed)),
                Err(PriceError::ParseFailure) => Err(make_parse_err(trimmed)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// State normalization (OnceCell lookup + fallback)
// ---------------------------------------------------------------------------

fn normalize_state(raw: &str) -> ProductState {
    let key = raw.trim().to_lowercase();
    state_map()
        .get(key.as_str())
        .copied()
        .unwrap_or(ProductState::Unknown)
}

// ---------------------------------------------------------------------------
// Image URL normalization
// ---------------------------------------------------------------------------

fn normalize_images(
    raw: Vec<String>,
    base_url: &Url,
) -> Result<Vec<ProductImage>, NormalizationError> {
    raw.into_iter()
        .map(|s| {
            let s = s.trim().to_owned();
            // Try parsing as-is first; fall back to joining with the base URL.
            let url = Url::parse(&s)
                .or_else(|_| base_url.join(&s))
                .map_err(|source| NormalizationError::InvalidImageUrl {
                    raw: s.clone(),
                    source,
                })?;
            Ok(ProductImage {
                url,
                prohibited_content: ProhibitedContent::Unknown,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// DateTime normalization
// ---------------------------------------------------------------------------

/// Attempts to parse a datetime string using a series of well-known formats.
///
/// Supported:
/// - RFC 3339 / ISO 8601 with offset  (`2024-06-01T10:00:00+02:00`)
/// - ISO 8601 date-only               (`2024-06-01`)              → midnight UTC
/// - `DD.MM.YYYY HH:MM`               (German / European)
/// - `DD.MM.YYYY`                     (German / European date-only)
/// - `MM/DD/YYYY HH:MM`               (US style)
/// - `MM/DD/YYYY`                     (US style date-only)
/// - `YYYY-MM-DD HH:MM:SS`            (SQL-style)
/// - `YYYY-MM-DD HH:MM`
/// - Unix epoch (integer seconds)
fn parse_datetime(raw: &str) -> Option<OffsetDateTime> {
    let s = raw.trim();

    // 1. RFC 3339
    if let Ok(dt) = OffsetDateTime::parse(s, &Rfc3339) {
        return Some(dt);
    }

    // 2. ISO 8601-like with space instead of T  e.g. "2024-06-01 10:00:00+02:00"
    if let Ok(dt) = OffsetDateTime::parse(&s.replacen(' ', "T", 1), &Rfc3339) {
        return Some(dt);
    }

    // 3. ISO 8601 date-only "YYYY-MM-DD"
    {
        let fmt = format_description!("[year]-[month]-[day]");
        if let Ok(date) = time::Date::parse(s, &fmt) {
            return Some(date.midnight().assume_utc());
        }
    }

    // 4. "YYYY-MM-DD HH:MM:SS" (no timezone → assume UTC)
    {
        let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 5. "YYYY-MM-DD HH:MM"
    {
        let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 6. "DD.MM.YYYY HH:MM:SS"
    {
        let fmt = format_description!("[day].[month].[year] [hour]:[minute]:[second]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 7. "DD.MM.YYYY HH:MM"
    {
        let fmt = format_description!("[day].[month].[year] [hour]:[minute]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 8. "DD.MM.YYYY"
    {
        let fmt = format_description!("[day].[month].[year]");
        if let Ok(date) = time::Date::parse(s, &fmt) {
            return Some(date.midnight().assume_utc());
        }
    }

    // 9. "MM/DD/YYYY HH:MM:SS"
    {
        let fmt = format_description!("[month]/[day]/[year] [hour]:[minute]:[second]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 10. "MM/DD/YYYY HH:MM"
    {
        let fmt = format_description!("[month]/[day]/[year] [hour]:[minute]");
        if let Ok(pdt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(pdt.assume_utc());
        }
    }

    // 11. "MM/DD/YYYY"
    {
        let fmt = format_description!("[month]/[day]/[year]");
        if let Ok(date) = time::Date::parse(s, &fmt) {
            return Some(date.midnight().assume_utc());
        }
    }

    // 12. Unix epoch (integer seconds)
    if let Ok(epoch) = s.parse::<i64>() {
        return OffsetDateTime::from_unix_timestamp(epoch).ok();
    }

    None
}

fn normalize_datetime_field(
    raw: Option<String>,
    make_err: impl Fn(String) -> NormalizationError,
) -> Result<Option<OffsetDateTime>, NormalizationError> {
    match raw {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim().to_owned();
            if trimmed.is_empty() {
                return Ok(None);
            }
            parse_datetime(&trimmed)
                .map(Some)
                .ok_or_else(|| make_err(trimmed))
        }
    }
}

// ---------------------------------------------------------------------------
// Title language detection (reuses description helper)
// ---------------------------------------------------------------------------

fn normalize_title_localized(raw: &str) -> Result<Localized<Language, Title>, NormalizationError> {
    let title = normalize_title(raw)?;
    let language = detect_language(title.as_ref()).ok_or_else(|| {
        NormalizationError::TitleUnknownLanguage {
            text: title.as_ref().chars().take(100).collect(),
        }
    })?;
    Ok(Localized::new(language, title))
}

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl ProductNormalizationService for ProductNormalizationServiceImpl {
    fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
    ) -> Result<NormalizedProduct, NormalizationError> {
        let shops_product_id = normalize_shops_product_id(&raw.shops_product_id)?;
        let title = normalize_title_localized(&raw.title)?;
        let description = normalize_description(raw.description)?;

        let price = normalize_price_field(
            raw.price,
            |r| NormalizationError::PriceUnknownCurrency { raw: r },
            |r| NormalizationError::PriceParseError { raw: r },
        )?;
        let price_estimate_min = normalize_price_field(
            raw.price_estimate_min,
            |r| NormalizationError::PriceEstimateMinUnknownCurrency { raw: r },
            |r| NormalizationError::PriceEstimateMinParseError { raw: r },
        )?;
        let price_estimate_max = normalize_price_field(
            raw.price_estimate_max,
            |r| NormalizationError::PriceEstimateMaxUnknownCurrency { raw: r },
            |r| NormalizationError::PriceEstimateMaxParseError { raw: r },
        )?;

        let state = normalize_state(&raw.state);
        let images = normalize_images(raw.images, &url)?;

        let auction_start = normalize_datetime_field(raw.auction_start, |r| {
            NormalizationError::AuctionStartParseError { raw: r }
        })?;
        let auction_end = normalize_datetime_field(raw.auction_end, |r| {
            NormalizationError::AuctionEndParseError { raw: r }
        })?;

        Ok(NormalizedProduct {
            shops_product_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            state,
            url,
            images,
            auction_start,
            auction_end,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::OffsetDateTime;
    use time::macros::datetime;
    use url::Url;

    use common::{
        currency::domain::Currency,
        price::domain::{MonetaryAmount, Price},
        product_state::domain::ProductState,
    };

    use super::{
        NormalizationError, PriceError, ProductNormalizationService,
        ProductNormalizationServiceImpl, detect_currency, normalize_description, normalize_images,
        normalize_shops_product_id, normalize_state, normalize_title, normalize_title_localized,
        parse_datetime, parse_price,
    };
    use crate::css_selector::product_schema::RawExtractedProduct;

    fn base_url() -> Url {
        Url::parse("https://example.com/products/123").unwrap()
    }

    fn minimal_raw() -> RawExtractedProduct {
        RawExtractedProduct {
            shops_product_id: "PROD-001".into(),
            // Long enough for whatlang to reliably identify as English.
            title: "Antique ceramic vase from the early twentieth century in excellent condition"
                .into(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "available".into(),
            images: vec![],
            auction_start: None,
            auction_end: None,
        }
    }

    // -----------------------------------------------------------------------
    // shops_product_id
    // -----------------------------------------------------------------------

    #[test]
    fn should_normalize_shops_product_id_when_plain_string_provided() {
        let result = normalize_shops_product_id("PROD-001").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_trim_whitespace_when_normalizing_shops_product_id() {
        let result = normalize_shops_product_id("  PROD-001  ").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_return_error_when_shops_product_id_is_empty() {
        let err = normalize_shops_product_id("").unwrap_err();
        assert!(matches!(err, NormalizationError::ShopsProductIdEmpty));
    }

    #[test]
    fn should_return_error_when_shops_product_id_is_only_whitespace() {
        let err = normalize_shops_product_id("   ").unwrap_err();
        assert!(matches!(err, NormalizationError::ShopsProductIdEmpty));
    }

    // -----------------------------------------------------------------------
    // title
    // -----------------------------------------------------------------------

    #[test]
    fn should_normalize_title_when_plain_string_provided() {
        let title = normalize_title("Antique Vase").unwrap();
        assert_eq!(title.as_ref(), "Antique Vase");
    }

    #[test]
    fn should_capitalize_first_letter_when_title_starts_lowercase() {
        // normalize_title only trims and capitalises — no language detection here.
        let title = normalize_title("antique vase").unwrap();
        assert_eq!(&title.as_ref()[..1], "A");
    }

    #[test]
    fn should_trim_whitespace_when_normalizing_title() {
        // normalize_title only trims — no language detection here.
        let title = normalize_title("  Antique Vase  ").unwrap();
        assert_eq!(title.as_ref(), "Antique Vase");
    }

    #[test]
    fn should_return_error_when_title_is_empty() {
        let err = normalize_title("").unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[test]
    fn should_return_error_when_title_is_only_whitespace() {
        let err = normalize_title("   ").unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[test]
    fn should_detect_language_for_title_when_english_text() {
        let localized = normalize_title_localized("This is an antique vase from England").unwrap();
        use common::language::domain::Language;
        assert_eq!(localized.localization, Language::En);
    }

    #[test]
    fn should_return_error_when_title_language_cannot_be_detected() {
        // A single character cannot be language-detected reliably.
        let err = normalize_title_localized("X").unwrap_err();
        assert!(
            matches!(err, NormalizationError::TitleUnknownLanguage { .. }),
            "expected TitleUnknownLanguage, got: {:?}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // description
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_none_when_description_fragments_are_empty() {
        let result = normalize_description(vec![]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_return_none_when_all_fragments_are_blank() {
        let result = normalize_description(vec!["  ".into(), "\t".into()]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_join_fragments_with_double_newline_when_multiple_fragments() {
        let result = normalize_description(vec![
            "This antique piece comes from a private English collection.".into(),
            "It was acquired during the early twentieth century by the original owner.".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_trim_each_fragment_when_fragments_have_surrounding_whitespace() {
        let result = normalize_description(vec![
            "  This antique piece comes from a private English collection.  ".into(),
            "  It was acquired during the early twentieth century by the owner.  ".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the owner."
        );
    }

    #[test]
    fn should_skip_blank_fragments_when_some_fragments_are_blank() {
        let result = normalize_description(vec![
            "This antique piece comes from a private English collection.".into(),
            "  ".into(),
            "It was acquired during the early twentieth century by the original owner.".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_return_single_paragraph_when_only_one_non_blank_fragment() {
        let result = normalize_description(vec![
            "This antique piece comes from a private English collection and dates to around nineteen twenty.".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection and dates to around nineteen twenty."
        );
    }

    #[test]
    fn should_return_error_when_description_language_cannot_be_detected() {
        // A single character cannot be language-detected reliably.
        let err = normalize_description(vec!["X".into()]).unwrap_err();
        assert!(
            matches!(err, NormalizationError::DescriptionUnknownLanguage { .. }),
            "expected DescriptionUnknownLanguage, got: {:?}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // price parsing
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
    fn should_parse_price_when_valid_string_provided(
        #[case] raw: &str,
        #[case] expected_amount: u64,
        #[case] expected_currency: Currency,
    ) {
        let (amount, currency) = parse_price(raw).unwrap();
        assert_eq!(*amount, expected_amount, "amount mismatch for '{}'", raw);
        assert_eq!(
            currency, expected_currency,
            "currency mismatch for '{}'",
            raw
        );
    }

    #[rstest]
    #[case("no numbers here USD")]
    #[case("€")]
    fn should_return_parse_failure_when_price_has_currency_but_no_number(#[case] raw: &str) {
        let result = parse_price(raw);
        assert!(
            matches!(result, Err(PriceError::ParseFailure)),
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
        let result = parse_price(raw);
        assert!(
            matches!(result, Err(PriceError::UnknownCurrency)),
            "expected UnknownCurrency for '{}'",
            raw
        );
    }

    // -----------------------------------------------------------------------
    // currency detection
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("$100", Some(Currency::Usd))]
    #[case("USD 100", Some(Currency::Usd))]
    #[case("£100", Some(Currency::Gbp))]
    #[case("GBP 100", Some(Currency::Gbp))]
    #[case("€ 100", Some(Currency::Eur))]
    #[case("EUR 100", Some(Currency::Eur))]
    #[case("100", None)]
    #[case("CHF 100", None)]
    #[case("A$100", Some(Currency::Aud))]
    #[case("AUD 100", Some(Currency::Aud))]
    #[case("C$100", Some(Currency::Cad))]
    #[case("CAD 100", Some(Currency::Cad))]
    #[case("NZ$100", Some(Currency::Nzd))]
    #[case("NZD 100", Some(Currency::Nzd))]
    fn should_detect_currency_when_symbol_or_code_present(
        #[case] raw: &str,
        #[case] expected: Option<Currency>,
    ) {
        assert_eq!(detect_currency(raw), expected);
    }

    // -----------------------------------------------------------------------
    // state normalization
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("available", ProductState::Available)]
    #[case("Available", ProductState::Available)]
    #[case("AVAILABLE", ProductState::Available)]
    #[case("in stock", ProductState::Available)]
    #[case("add to cart", ProductState::Available)]
    #[case("buy now", ProductState::Available)]
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
    #[case("reserviert", ProductState::Reserved)]
    #[case("verkauft", ProductState::Sold)]
    #[case("ausverkauft", ProductState::Sold)]
    // French
    #[case("disponible", ProductState::Available)]
    #[case("vendu", ProductState::Sold)]
    #[case("listé", ProductState::Listed)]
    // Italian
    #[case("inserito", ProductState::Listed)]
    #[case("riservato", ProductState::Reserved)]
    // Unknown
    #[case("some random text", ProductState::Unknown)]
    #[case("", ProductState::Unknown)]
    fn should_normalize_state_when_raw_value_provided(
        #[case] raw: &str,
        #[case] expected: ProductState,
    ) {
        assert_eq!(normalize_state(raw), expected);
    }

    // -----------------------------------------------------------------------
    // image normalization
    // -----------------------------------------------------------------------

    #[test]
    fn should_normalize_images_when_absolute_urls_provided() {
        let images = vec!["https://cdn.example.com/img1.jpg".into()];
        let result = normalize_images(images, &base_url()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url.as_str(), "https://cdn.example.com/img1.jpg");
    }

    #[test]
    fn should_normalize_images_when_relative_urls_provided() {
        let images = vec!["/images/item.jpg".into()];
        let result = normalize_images(images, &base_url()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].url.as_str(),
            "https://example.com/images/item.jpg"
        );
    }

    #[test]
    fn should_return_empty_vec_when_no_images_provided() {
        let result = normalize_images(vec![], &base_url()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn should_return_error_when_image_url_is_invalid() {
        // A string with a scheme-like prefix that is invalid as an absolute URL
        // and also fails as a relative URL join because it starts with `//`
        // followed by an invalid host (empty authority).
        let images = vec!["//".into()];
        let err = normalize_images(images, &base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::InvalidImageUrl { .. }));
    }

    #[test]
    fn should_set_prohibited_content_to_unknown_when_images_normalized() {
        use product::core::prohibited_content::ProhibitedContent;
        let images = vec!["https://cdn.example.com/img.jpg".into()];
        let result = normalize_images(images, &base_url()).unwrap();
        assert_eq!(result[0].prohibited_content, ProhibitedContent::Unknown);
    }

    #[test]
    fn should_trim_whitespace_from_image_urls_when_normalizing() {
        let images = vec!["  https://cdn.example.com/img.jpg  ".into()];
        let result = normalize_images(images, &base_url()).unwrap();
        assert_eq!(result[0].url.as_str(), "https://cdn.example.com/img.jpg");
    }

    // -----------------------------------------------------------------------
    // datetime parsing
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01T10:00:00+02:00", datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-06-01T10:00:00Z", datetime!(2024-06-01 10:00:00 UTC))]
    #[case("2024-06-01T00:00:00Z", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-06-01 10:00:00+02:00", datetime!(2024-06-01 10:00:00 +2))]
    fn should_parse_datetime_when_rfc3339_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("2024-06-01", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-12-31", datetime!(2024-12-31 00:00:00 UTC))]
    fn should_parse_datetime_when_iso_date_only_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("2024-06-01 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("2024-06-01 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    fn should_parse_datetime_when_sql_style_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("01.06.2024 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("01.06.2024 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("01.06.2024", datetime!(2024-06-01 00:00:00 UTC))]
    fn should_parse_datetime_when_german_format_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("06/01/2024 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("06/01/2024 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("06/01/2024", datetime!(2024-06-01 00:00:00 UTC))]
    fn should_parse_datetime_when_us_format_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("1717228800")] // 2024-06-01T00:00:00Z
    fn should_parse_datetime_when_unix_epoch_string_provided(#[case] raw: &str) {
        let result = parse_datetime(raw);
        assert!(result.is_some(), "expected Some for unix epoch '{}'", raw);
        assert_eq!(result.unwrap().unix_timestamp(), 1717228800);
    }

    #[rstest]
    #[case("not a date")]
    #[case("32.13.2024")]
    #[case("")]
    fn should_return_none_when_datetime_string_is_unparseable(#[case] raw: &str) {
        assert_eq!(parse_datetime(raw), None);
    }

    // -----------------------------------------------------------------------
    // Full normalize() integration
    // -----------------------------------------------------------------------

    #[test]
    fn should_normalize_product_when_minimal_raw_provided() {
        let svc = ProductNormalizationServiceImpl::new();
        let raw = minimal_raw();
        let result = svc.normalize(raw, base_url()).unwrap();

        assert_eq!(result.shops_product_id.to_string(), "PROD-001");
        assert_eq!(
            result.title.payload.as_ref(),
            "Antique ceramic vase from the early twentieth century in excellent condition"
        );
        assert!(result.description.is_none());
        assert!(result.price.is_none());
        assert!(result.price_estimate_min.is_none());
        assert!(result.price_estimate_max.is_none());
        assert_eq!(result.state, ProductState::Available);
        assert!(result.images.is_empty());
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
    }

    #[test]
    fn should_normalize_product_when_full_raw_provided() {
        let svc = ProductNormalizationServiceImpl::new();
        let raw = RawExtractedProduct {
            shops_product_id: "LOT-42".into(),
            // Long enough English text for reliable language detection.
            title:
                "Victorian silver brooch in excellent original condition from private collection"
                    .into(),
            description: vec![
                "A beautiful antique brooch from the Victorian era.".into(),
                "In excellent original condition with no damage.".into(),
            ],
            price: Some("€ 1.200,00".into()),
            price_estimate_min: Some("£ 800.00".into()),
            price_estimate_max: Some("£1,200.00".into()),
            state: "listed".into(),
            images: vec![
                "https://cdn.example.com/img1.jpg".into(),
                "/img2.jpg".into(),
            ],
            auction_start: Some("2024-06-01T10:00:00Z".into()),
            auction_end: Some("2024-07-01T10:00:00Z".into()),
        };

        let result = svc.normalize(raw, base_url()).unwrap();

        assert_eq!(result.shops_product_id.to_string(), "LOT-42");
        assert_eq!(
            result.title.payload.as_ref(),
            "Victorian silver brooch in excellent original condition from private collection"
        );
        assert_eq!(
            result.description.unwrap().payload.as_ref(),
            "A beautiful antique brooch from the Victorian era.\n\nIn excellent original condition with no damage."
        );
        assert_eq!(
            result.price.unwrap(),
            Price::new(MonetaryAmount::from(120000u64), Currency::Eur)
        );
        assert_eq!(
            result.price_estimate_min.unwrap(),
            Price::new(MonetaryAmount::from(80000u64), Currency::Gbp)
        );
        assert_eq!(
            result.price_estimate_max.unwrap(),
            Price::new(MonetaryAmount::from(120000u64), Currency::Gbp)
        );
        assert_eq!(result.state, ProductState::Listed);
        assert_eq!(result.images.len(), 2);
        assert_eq!(
            result.auction_start.unwrap(),
            datetime!(2024-06-01 10:00:00 UTC)
        );
        assert_eq!(
            result.auction_end.unwrap(),
            datetime!(2024-07-01 10:00:00 UTC)
        );
    }

    #[test]
    fn should_return_error_when_shops_product_id_is_empty_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.shops_product_id = "  ".into();
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::ShopsProductIdEmpty));
    }

    #[test]
    fn should_return_error_when_title_is_empty_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.title = "".into();
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[test]
    fn should_return_error_when_price_has_no_currency_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price = Some("1234.56".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceUnknownCurrency { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price = Some("€".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::PriceParseError { .. }));
    }

    #[test]
    fn should_return_error_when_price_estimate_min_has_no_currency_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("800.00".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMinUnknownCurrency { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_estimate_min_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_min = Some("£".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMinParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_estimate_max_has_no_currency_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("1200".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMaxUnknownCurrency { .. }
        ));
    }

    #[test]
    fn should_return_error_when_price_estimate_max_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price_estimate_max = Some("£".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::PriceEstimateMaxParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_auction_start_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.auction_start = Some("yesterday at noon".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::AuctionStartParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_auction_end_is_unparseable_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.auction_end = Some("next tuesday".into());
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::AuctionEndParseError { .. }
        ));
    }

    #[test]
    fn should_return_error_when_image_url_is_invalid_for_normalize() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.images = vec!["//".into()];
        let err = svc.normalize(raw, base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::InvalidImageUrl { .. }));
    }

    #[test]
    fn should_use_url_from_argument_as_product_url_when_normalizing() {
        let svc = ProductNormalizationServiceImpl::new();
        let url = Url::parse("https://shop.example.com/item/99").unwrap();
        let result = svc.normalize(minimal_raw(), url.clone()).unwrap();
        assert_eq!(result.url, url);
    }

    #[test]
    fn should_fallback_to_unknown_state_when_raw_state_not_in_lookup_table() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.state = "some_totally_unknown_state_xyz".into();
        let result = svc.normalize(raw, base_url()).unwrap();
        assert_eq!(result.state, ProductState::Unknown);
    }

    #[test]
    fn should_skip_none_price_fields_when_raw_prices_are_absent() {
        let svc = ProductNormalizationServiceImpl::new();
        let result = svc.normalize(minimal_raw(), base_url()).unwrap();
        assert!(result.price.is_none());
        assert!(result.price_estimate_min.is_none());
        assert!(result.price_estimate_max.is_none());
    }

    #[test]
    fn should_handle_empty_optional_price_string_when_raw_price_is_blank() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.price = Some("  ".into());
        // Blank string treated as absent — no currency error expected.
        let result = svc.normalize(raw, base_url()).unwrap();
        assert!(result.price.is_none());
    }

    #[test]
    fn should_handle_empty_optional_auction_string_when_raw_auction_is_blank() {
        let svc = ProductNormalizationServiceImpl::new();
        let mut raw = minimal_raw();
        raw.auction_start = Some("  ".into());
        raw.auction_end = Some("  ".into());
        let result = svc.normalize(raw, base_url()).unwrap();
        assert!(result.auction_start.is_none());
        assert!(result.auction_end.is_none());
    }
}
