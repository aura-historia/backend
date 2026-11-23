//! Asking-price ranges in display text: `€ 1.000 – 1.500`, `1.000 - 1.500 EUR`,
//! `€1.250 - €2.500`, `from £1,200 to £1,500`, `ab 1.000 bis 1.500 €`, `1,000円～1,500円`.
//!
//! A range layout is two consecutive amounts joined by a dash, a tilde or a range word. The
//! currency is stated once for both bounds, before the lower (`€ 1.000 – 1.500`) or after the
//! upper (`1.000 – 1.500 €`), or on each bound. Each bound is one written number under the
//! rules of a single amount (see [`super::display_number`]).
//!
//! A currency written only between the bounds belongs to its own amount, so the other number
//! is no bound: `1.250 € - 3 Stück` and `Lot 12 – € 680` are no ranges and stay single prices.
//! A currency on one side covers only a bare number on the other: a number a word names or
//! qualifies (`Lot 12 – 680 €`, `€ 45 - 100 Stück`) or one written with a leading zero is no
//! bound either. A dash directly after a decimal separator is a sign or a zero-cents dash
//! (`100,-200`, `2490,- €`), never a range separator. A mix of currencies the fallback currency
//! names is a dual-currency display, which the single-price rule reads.
//!
//! Text that holds a range layout but cannot be one range fails closed with
//! [`PriceError::InvalidRange`]: mixed currencies, a lower bound that is not below the upper
//! one, several different ranges, or a range next to another price.

use super::currency_marker::{CurrencyMarker, currency_markers, has_unsupported_dollar};
use super::display_number::{PriceNumberCandidate, extract_price_number_candidates, stands_alone};
use super::{
    PriceError, euro_sign_decimal_number, has_amount_run_into_a_percentage, has_negative_sign,
    parse_price_candidate, price_currency_pairs,
};
use money::Currency;
use product_listing_core::product_listing_price::PriceRange;
use regex::regex;

/// A range a display text states, and whether the text names its currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisplayRange {
    pub(super) range: PriceRange,
    pub(super) explicit_currency: bool,
}

/// The whitespace a currency marker may stand apart from its amount, as for a single price.
const MAX_MARKER_GAP_CHARS: usize = 3;
/// The whitespace around a range separator. Extracted text keeps line breaks and indentation
/// of the markup, so this is wider than the marker gap.
const MAX_SEPARATOR_GAP_CHARS: usize = 8;

/// Reads the one range `raw` states. `Ok(None)` when the text holds no range layout, so it is
/// read as a single price.
pub(super) fn display_range(
    raw: &str,
    fallback_currency: Option<Currency>,
) -> Result<Option<DisplayRange>, PriceError> {
    // Layouts a single price owns or rejects as a whole are no ranges.
    if euro_sign_decimal_number(raw).is_some()
        || has_amount_run_into_a_percentage(raw)
        || has_unsupported_dollar(raw)
    {
        return Ok(None);
    }
    let candidates = extract_price_number_candidates(raw);
    let markers = currency_markers(raw);
    let mut ranged = Vec::new();
    let mut unranged = Vec::new();
    for layout in candidates
        .windows(2)
        .filter_map(|pair| RangeLayout::find(raw, &pair[0], &pair[1], &markers))
    {
        match layout.read(raw, &markers, fallback_currency) {
            LayoutReading::NoRange => unranged.push(layout),
            LayoutReading::Range(range) => ranged.push((layout, range?)),
        }
    }
    let Some(&(_, first)) = ranged.first() else {
        return Ok(None);
    };

    let is_bound = |start: usize| ranged.iter().any(|(layout, _)| layout.has_bound_at(start));
    // Several different ranges, a bound joined to a further number (`€ 100 - 200 - 300`) and a
    // range next to another price (`Was € 2.000 Now € 1.000 – 1.500`) are ambiguous.
    let is_ambiguous = ranged.iter().any(|(_, range)| *range != first)
        || unranged
            .iter()
            .any(|layout| is_bound(layout.lower.start) || is_bound(layout.upper.start))
        || price_currency_pairs(raw)
            .iter()
            .any(|pair| !is_bound(pair.candidate.start));
    if is_ambiguous {
        return Err(PriceError::InvalidRange);
    }
    Ok(Some(first))
}

/// Two consecutive amounts joined as a range, with the currency markers and words around them.
struct RangeLayout<'a> {
    lower: &'a PriceNumberCandidate,
    upper: &'a PriceNumberCandidate,
    /// Before the lower bound (`€ 1.000 – 1.500`).
    leading: Option<CurrencyMarker>,
    /// After the lower bound (`1.000 € – 1.500 €`).
    after_lower: Option<CurrencyMarker>,
    /// Before the upper bound (`€ 1.000 – € 1.500`).
    before_upper: Option<CurrencyMarker>,
    /// After the upper bound (`1.000 – 1.500 €`).
    trailing: Option<CurrencyMarker>,
    /// Whether a minus sign stands on the upper bound or its currency.
    upper_signed: bool,
    /// Where the layout starts, including an opening word (`from`, `ab`, `entre`).
    start: usize,
    end: usize,
}

enum LayoutReading {
    /// The layout is no range of this text, such as two numbers without a currency.
    NoRange,
    Range(Result<DisplayRange, PriceError>),
}

impl<'a> RangeLayout<'a> {
    fn find(
        raw: &str,
        lower: &'a PriceNumberCandidate,
        upper: &'a PriceNumberCandidate,
        markers: &[CurrencyMarker],
    ) -> Option<Self> {
        let mut cursor = zero_cents_dash_end(raw, lower.end);
        let gap = whitespace_end(raw, cursor, MAX_SEPARATOR_GAP_CHARS)?;
        let after_lower = marker_at(markers, gap)
            .filter(|_| whitespace_end(raw, cursor, MAX_MARKER_GAP_CHARS).is_some());
        if let Some(marker) = after_lower {
            cursor = marker.end;
        }
        let separator_start = whitespace_end(raw, cursor, MAX_SEPARATOR_GAP_CHARS)?;
        let (separator, separator_end) =
            Separator::at(raw, separator_start, separator_start > cursor)?;
        let before_upper_start = whitespace_end(raw, separator_end, MAX_SEPARATOR_GAP_CHARS)?;
        if separator.is_word() && before_upper_start == separator_end {
            return None;
        }
        // A sign on the upper bound or its currency keeps the layout, so the bound is rejected
        // as negative rather than the lower bound read as a single price. The separator itself
        // is never a sign (`1.000€-1.500€`).
        let signed_start = sign_end(raw, before_upper_start);
        let (before_upper, number_start) = match marker_at(markers, signed_start) {
            Some(marker) => (
                Some(marker),
                whitespace_end(raw, marker.end, MAX_MARKER_GAP_CHARS)?,
            ),
            None => (None, before_upper_start),
        };
        let unsigned_start = sign_end(raw, number_start);
        if unsigned_start != upper.start {
            return None;
        }
        // A minus spaced off the lower bound but written onto the upper one (`€ 1.000 -1.500`)
        // reads as a sign.
        let sign_like_separator = separator == Separator::Dash
            && raw[separator_start..separator_end].chars().all(is_minus)
            && separator_start > cursor
            && before_upper_start == separator_end;
        let upper_signed = sign_like_separator
            || signed_start != before_upper_start
            || unsigned_start != number_start;

        let leading = markers.iter().copied().find(|marker| {
            marker.end <= lower.start
                && is_whitespace_gap(
                    raw[marker.end..lower.start].trim_end_matches(is_minus),
                    MAX_MARKER_GAP_CHARS,
                )
        });
        let upper_end = zero_cents_dash_end(raw, upper.end);
        let trailing = whitespace_end(raw, upper_end, MAX_MARKER_GAP_CHARS)
            .and_then(|start| marker_at(markers, start));

        let bounds_start = leading.map_or(lower.start, |marker| marker.start);
        let opener = opener_start(&raw[..bounds_start]);
        if separator.needs_between_opener() && !opener.is_some_and(|(_, between)| between) {
            return None;
        }
        Some(Self {
            lower,
            upper,
            leading,
            after_lower,
            before_upper,
            trailing,
            upper_signed,
            start: opener.map_or(bounds_start, |(start, _)| start),
            end: trailing.map_or(upper_end, |marker| marker.end),
        })
    }

    fn has_bound_at(&self, start: usize) -> bool {
        self.lower.start == start || self.upper.start == start
    }

    fn markers(&self) -> impl Iterator<Item = CurrencyMarker> {
        [
            self.leading,
            self.after_lower,
            self.before_upper,
            self.trailing,
        ]
        .into_iter()
        .flatten()
    }

    fn read(
        &self,
        raw: &str,
        markers: &[CurrencyMarker],
        fallback_currency: Option<Currency>,
    ) -> LayoutReading {
        // A currency written only between the bounds belongs to its own amount: the other
        // number, without a currency of its own, is no bound.
        let lower_has_currency = self.leading.is_some() || self.after_lower.is_some();
        let upper_has_currency = self.before_upper.is_some() || self.trailing.is_some();
        if (self.after_lower.is_some() && !upper_has_currency)
            || (self.before_upper.is_some() && !lower_has_currency)
        {
            return LayoutReading::NoRange;
        }
        // A currency on one side covers a bare number on the other side only. A number a word
        // names or qualifies (`Lot 12 – 680 €`, `€ 45 - 100 Stück`, `$45 - 1 left`) and a
        // number written with a leading zero (`0171`) are no amounts of the range.
        // A bound that continues as another number form (`1,5 Mio.`) stays, so it fails closed.
        if [self.lower, self.upper]
            .iter()
            .any(|bound| has_leading_zero(&bound.value))
            || (!lower_has_currency
                && stands_alone(raw, self.lower)
                && !is_bare_before(&raw[..self.start]))
            || (!upper_has_currency
                && stands_alone(raw, self.upper)
                && !is_bare_after(&raw[self.end..]))
        {
            return LayoutReading::NoRange;
        }

        let mut stated = self.markers().map(|marker| marker.currency);
        let (currency, explicit_currency) = match stated.next() {
            Some(currency) => match stated.find(|other| *other != currency) {
                // A dual-currency display (`€ 35.00 - CHF 33.00`) is the single-price rule's
                // to read with the fallback currency; any other mix is no range.
                Some(other) if fallback_currency.is_some_and(|f| f == currency || f == other) => {
                    return LayoutReading::NoRange;
                }
                Some(_) => return LayoutReading::Range(Err(PriceError::InvalidRange)),
                None => (currency, true),
            },
            None => match fallback_currency {
                Some(fallback) if self.is_whole_price_text(raw, markers, fallback) => {
                    (fallback, false)
                }
                _ => return LayoutReading::NoRange,
            },
        };
        LayoutReading::Range(self.bounds(raw, currency).map(|range| DisplayRange {
            range,
            explicit_currency,
        }))
    }

    fn bounds(&self, raw: &str, currency: Currency) -> Result<PriceRange, PriceError> {
        if has_negative_sign(raw, self.lower) || self.upper_signed {
            return Err(PriceError::NegativeAmount);
        }
        let lower = parse_price_candidate(raw, self.lower, &currency)?;
        let upper = parse_price_candidate(raw, self.upper, &currency)?;
        PriceRange::from_amounts(currency, lower, upper).map_err(|_| PriceError::InvalidRange)
    }

    /// Without a currency marker, two numbers are a range only when they are the whole price
    /// text, apart from a price label (`Preis: 1.000 – 1.500`) and the fallback currency itself,
    /// so that a delivery time or a reference (`Lieferzeit: 1 - 2`, `Art.-Nr.: 12-34`) never
    /// becomes a price.
    fn is_whole_price_text(
        &self,
        raw: &str,
        markers: &[CurrencyMarker],
        fallback: Currency,
    ) -> bool {
        if markers.iter().any(|marker| marker.currency != fallback) {
            return false;
        }
        let outside_markers = |text: &str, offset: usize| {
            text.char_indices().all(|(index, ch)| {
                !ch.is_alphanumeric()
                    || markers
                        .iter()
                        .any(|marker| (marker.start..marker.end).contains(&(offset + index)))
            })
        };
        let before = &raw[..self.start];
        (outside_markers(before, 0) || is_price_label(before))
            && outside_markers(&raw[self.end..], self.end)
    }
}

/// Whether `before` is nothing but a price label (`Preis:`, `Price`), with punctuation.
fn is_price_label(before: &str) -> bool {
    regex!(r"(?i)^[^\p{L}\p{N}]*(?:price|our\s+price|sale\s+price|preis|kaufpreis|verkaufspreis|prix|prezzo|precio|preço|preco|prijs|cena|pris|hinta|fiyat)\s*:?[^\p{L}\p{N}]*$")
        .is_match(before)
}

/// Whether the text before a lower bound without its own currency leaves it a bare amount: it
/// ends in punctuation or a price label, not in a word that names the number (`Lot 12`,
/// `Art. 4711`).
fn is_bare_before(before: &str) -> bool {
    let word = before.trim_end().trim_end_matches(['.', ':']);
    word.chars().next_back().is_none_or(|ch| !ch.is_alphanumeric())
        || regex!(r"(?i)(?:^|[^\p{L}])(?:price|our\s+price|sale\s+price|preis|kaufpreis|verkaufspreis|prix|prezzo|precio|preço|preco|prijs|cena|pris|hinta|fiyat|ca|circa|approx|approximately|now|jetzt|nur|only)$")
            .is_match(word)
}

/// Whether the text after an upper bound without its own currency leaves it a bare amount: it
/// starts with punctuation or a tax or negotiation note, not with a unit or count (`100 Stück`).
fn is_bare_after(after: &str) -> bool {
    let after = after.trim_start();
    after.chars().next().is_none_or(|ch| !ch.is_alphanumeric())
        || regex!(r"(?i)^(?:inkl|incl|exkl|excl|zzgl|zzg|plus|vat|mwst|ust|ttc|ht|iva|tax|vb|ono|obo)(?:[^\p{L}]|$)")
            .is_match(after)
}

/// Whether a number is written with a leading zero (`0171`), as references and phone numbers
/// are and amounts are not.
fn has_leading_zero(number: &str) -> bool {
    let mut digits = number.chars();
    digits.next() == Some('0') && digits.next().is_some_and(|ch| ch.is_ascii_digit())
}

/// What joins the bounds of a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Separator {
    /// A dash or a tilde: `1.000 – 1.500`, `1,000円～1,500円`.
    Dash,
    /// A word that reads as `to`: `1.000 bis 1.500 €`, `from £1,200 to £1,500`.
    To,
    /// A word that reads as `and`, a range only after `between`: `entre 1 000 et 1 500 €`.
    And,
}

impl Separator {
    /// The separator starting at `start`. A word needs whitespace on both sides, which
    /// `preceded_by_space` and the caller check.
    fn at(raw: &str, start: usize, preceded_by_space: bool) -> Option<(Self, usize)> {
        let rest = &raw[start..];
        let first = rest.chars().next()?;
        if is_range_dash(first) {
            return Some((Self::Dash, start + first.len_utf8()));
        }
        if !preceded_by_space {
            return None;
        }
        let word = regex!(
            r"^(?i:to|till?|bis|tot|à|au|a|al|hasta|até|ate|do|and|und|et|e|y|en|och|og|i)(?:\s|$)"
        )
        .find(rest)?
        .as_str()
        .trim_end();
        let separator = match word.to_lowercase().as_str() {
            "and" | "und" | "et" | "e" | "y" | "en" | "och" | "og" | "i" => Self::And,
            _ => Self::To,
        };
        Some((separator, start + word.len()))
    }

    fn is_word(self) -> bool {
        self != Self::Dash
    }

    fn needs_between_opener(self) -> bool {
        self == Self::And
    }
}

/// Dashes, minus forms and tildes written between the bounds of a range. A minus attached to a
/// bound still makes it negative (see [`has_negative_sign`]).
fn is_range_dash(ch: char) -> bool {
    matches!(
        ch,
        '-' | '\u{2010}'
            ..='\u{2015}'
                | '\u{2212}'
                | '\u{fe58}'
                | '\u{fe63}'
                | '\u{ff0d}'
                | '~'
                | '\u{2053}'
                | '\u{301c}'
                | '\u{ff5e}'
    )
}

/// The start of the word that opens a range (`from`, `ab`, `entre`) right before `before`
/// ends, and whether it reads as `between`, which an `and` separator needs.
fn opener_start(before: &str) -> Option<(usize, bool)> {
    let captures = regex!(
        r"(?i)(?:^|[^\p{L}\p{N}])(from|between|ab|von|zwischen|de|des|entre|da|dal|dai|tra|fra|desde|van|tussen|från|mellan|mellem|od|między)\s{1,8}$"
    )
    .captures(before)?;
    let word = captures.get(1)?;
    let between = matches!(
        word.as_str().to_lowercase().as_str(),
        "between"
            | "zwischen"
            | "entre"
            | "tra"
            | "fra"
            | "tussen"
            | "mellan"
            | "mellem"
            | "między"
    );
    Some((word.start(), between))
}

/// After a whole amount written with a dash in place of the cents (`1.000,– – 1.500,– €`),
/// the position past that dash. A dash followed by a digit is a sign or a separator
/// (`100,-200`), so it stays.
fn zero_cents_dash_end(raw: &str, end: usize) -> usize {
    regex!(r"^[.,](?:--|[-\u{2013}\u{2014}])")
        .find(&raw[end..])
        .filter(|dash| {
            raw[end + dash.end()..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_ascii_digit())
        })
        .map_or(end, |dash| end + dash.end())
}

/// The position past at most `max_chars` whitespace characters from `start`, or `None` when
/// more whitespace follows.
fn whitespace_end(raw: &str, start: usize, max_chars: usize) -> Option<usize> {
    let rest = &raw[start..];
    let gap = rest
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(rest.len());
    is_whitespace_gap(&rest[..gap], max_chars).then_some(start + gap)
}

fn is_whitespace_gap(gap: &str, max_chars: usize) -> bool {
    gap.chars().all(char::is_whitespace) && gap.chars().nth(max_chars).is_none()
}

fn is_minus(ch: char) -> bool {
    matches!(ch, '-' | '\u{2212}')
}

/// The position past one minus sign at `start`, if one is written there.
fn sign_end(raw: &str, start: usize) -> usize {
    raw[start..]
        .chars()
        .next()
        .filter(|ch| is_minus(*ch))
        .map_or(start, |ch| start + ch.len_utf8())
}

fn marker_at(markers: &[CurrencyMarker], start: usize) -> Option<CurrencyMarker> {
    markers.iter().copied().find(|marker| marker.start == start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn range(currency: Currency, min: u64, max: u64) -> PriceRange {
        PriceRange::from_amounts(currency, min, max).expect("valid test range")
    }

    #[rstest]
    // Issue #3107: the layouts that read one bound or failed before.
    #[case("€ 1.000 - 1.500", Currency::Eur, 100_000, 150_000)]
    #[case("1.000 - 1.500 EUR", Currency::Eur, 100_000, 150_000)]
    #[case("€1.250 - €2.500", Currency::Eur, 125_000, 250_000)]
    // Dashes, spacing and currency on one or both bounds.
    #[case("€ 1.000 – 1.500", Currency::Eur, 100_000, 150_000)]
    #[case("€ 1.000—1.500", Currency::Eur, 100_000, 150_000)]
    #[case("€1.000-1.500", Currency::Eur, 100_000, 150_000)]
    #[case("1.000- 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("1.000 € – 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("EUR 1.000 - EUR 1.500", Currency::Eur, 100_000, 150_000)]
    #[case("€ 1.000 – 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("$1,200 – $1,500", Currency::Usd, 120_000, 150_000)]
    #[case("US$ 1,200 - 1,500", Currency::Usd, 120_000, 150_000)]
    #[case("£1,200.50 – £1,500", Currency::Gbp, 120_050, 150_000)]
    #[case("CHF 1'200.– – 1'500.–", Currency::Chf, 120_000, 150_000)]
    #[case("1.000,– – 1.500,– €", Currency::Eur, 100_000, 150_000)]
    #[case("1.000,- - 1.500,- €", Currency::Eur, 100_000, 150_000)]
    #[case("1 000 – 1 500 €", Currency::Eur, 100_000, 150_000)]
    #[case(
        "1\u{202f}000\u{a0}–\u{a0}1\u{202f}500\u{a0}€",
        Currency::Eur,
        100_000,
        150_000
    )]
    #[case("1,000円～1,500円", Currency::Jpy, 1_000, 1_500)]
    #[case("1,000〜1,500円", Currency::Jpy, 1_000, 1_500)]
    #[case("€ 1.000 ~ 1.500", Currency::Eur, 100_000, 150_000)]
    #[case("€ 1.000\n    –\n    1.500", Currency::Eur, 100_000, 150_000)]
    // Range words, with and without an opening word.
    #[case("from £1,200 to £1,500", Currency::Gbp, 120_000, 150_000)]
    #[case("$1,200 to $1,500", Currency::Usd, 120_000, 150_000)]
    #[case("between $1,200 and $1,500", Currency::Usd, 120_000, 150_000)]
    #[case("ab 1.000 bis 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("von 1.000 € bis 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("1.000 bis 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("zwischen 1.000 und 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("entre 1 000 et 1 500 €", Currency::Eur, 100_000, 150_000)]
    #[case("de 1 000 € à 1 500 €", Currency::Eur, 100_000, 150_000)]
    #[case("da 1.000 a 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("tra 1.000 e 1.500 euro", Currency::Eur, 100_000, 150_000)]
    #[case("desde 1.000 hasta 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("van € 1.000 tot € 1.500", Currency::Eur, 100_000, 150_000)]
    #[case("FROM £1,200 TO £1,500", Currency::Gbp, 120_000, 150_000)]
    // Labels, VAT notes and badges around the range.
    #[case(
        "Preis: € 1.000 – 1.500 inkl. 19% MwSt.",
        Currency::Eur,
        100_000,
        150_000
    )]
    #[case(
        "Art.-Nr. 02092 Preis 1.000 - 1.500 €",
        Currency::Eur,
        100_000,
        150_000
    )]
    #[case(
        "Price range: $1,200 - $1,500 (30% off)",
        Currency::Usd,
        120_000,
        150_000
    )]
    #[case("1.000€-1.500€", Currency::Eur, 100_000, 150_000)]
    #[case("1.000 €-1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("Preis 1.000 - 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("ca. 1.000 - 1.500 €", Currency::Eur, 100_000, 150_000)]
    #[case("€ 1.000 - 1.500 VB", Currency::Eur, 100_000, 150_000)]
    #[case("€ 1.000 - 1.500 zzgl. Versand", Currency::Eur, 100_000, 150_000)]
    // The range the crawler writes for a JSON-LD AggregateOffer.
    #[case("1000.00 - 1500.00 EUR", Currency::Eur, 100_000, 150_000)]
    // A repeated range from a scraped fragment is one range.
    #[case("€1.000 - €1.500 €1.000 - €1.500", Currency::Eur, 100_000, 150_000)]
    fn should_read_a_range_when_the_text_states_one(
        #[case] raw: &str,
        #[case] currency: Currency,
        #[case] min: u64,
        #[case] max: u64,
    ) {
        for fallback in [None, Some(Currency::Eur)] {
            assert_eq!(
                Ok(Some(DisplayRange {
                    range: range(currency, min, max),
                    explicit_currency: true,
                })),
                display_range(raw, fallback),
                "{raw} with {fallback:?}"
            );
        }
    }

    #[rstest]
    #[case("1.000 - 1.500")]
    #[case("Preis: 1.000 – 1.500")]
    #[case("1.000 bis 1.500")]
    #[case("1.000 - 1.500 (EUR)")]
    fn should_read_a_whole_text_range_in_the_fallback_currency(#[case] raw: &str) {
        assert_eq!(
            Ok(Some(DisplayRange {
                range: range(Currency::Eur, 100_000, 150_000),
                explicit_currency: false,
            })),
            display_range(raw, Some(Currency::Eur)),
            "{raw}"
        );
    }

    #[test]
    fn should_read_a_json_ld_range_without_currency_in_the_fallback_currency() {
        assert_eq!(
            Ok(Some(DisplayRange {
                range: range(Currency::Usd, 4_000, 9_550),
                explicit_currency: false,
            })),
            display_range("40 - 95.5", Some(Currency::Usd))
        );
    }

    #[rstest]
    // No currency to read the numbers in.
    #[case("1.000 - 1.500", None)]
    // Numbers that are not the whole price text are no price range.
    #[case("Lieferzeit 1 bis 2 Wochen", Some(Currency::Eur))]
    #[case("1.000 - 1.500 inkl. MwSt.", Some(Currency::Eur))]
    #[case("Art. 12-34", Some(Currency::Eur))]
    // A stated currency other than the fallback is never overruled.
    #[case("1.000 - 1.500 (CHF)", Some(Currency::Eur))]
    // A currency between the bounds belongs to its own amount.
    #[case("1.250 € - 3 Stück", None)]
    #[case("Lot 12 – € 680", None)]
    #[case("1.000 € - 1.500", None)]
    // A word names or qualifies the number on the side without a currency.
    #[case("Lot 12 – 680 €", None)]
    #[case("Lot de 6 - 120 €", None)]
    #[case("Art. 4711 - 8.500 €", None)]
    #[case("€ 45 - 100 Stück", None)]
    #[case("€ 1.250 - 3 Stück", None)]
    #[case("$45.00 - 1 left in stock", None)]
    #[case("€ 3,50 al 100 g", None)]
    #[case("€ 850 - 1920er", None)]
    // A number with a leading zero is a reference, not an amount.
    #[case("Tel. 0171-2345678 €", None)]
    // The fallback currency reads no reference or delivery time as a range.
    #[case("Art.-Nr.: 12-34", Some(Currency::Eur))]
    #[case("Lieferzeit: 1 - 2", Some(Currency::Eur))]
    #[case("Tel.: 0171-2345678", Some(Currency::Eur))]
    #[case("Lieferzeit 3 Wochen, Maße: 40 - 60", Some(Currency::Eur))]
    // A dual-currency display the fallback currency resolves.
    #[case("€ 35.00 - CHF 33.00", Some(Currency::Chf))]
    #[case("€ 1.000 - 1.500 CHF", Some(Currency::Eur))]
    // A dash after a decimal separator is a sign or zero cents, not a range.
    #[case("100,-200 €", None)]
    #[case("1.000,--1.500 €", None)]
    #[case("Preis: 1690,-€ - RESERVIERT", None)]
    // Word hyphens and dashes before text.
    #[case("Vase - €100", None)]
    #[case("€12,500 – contact us to buy", None)]
    #[case("€ 100 and 200", None)]
    #[case("€ 1.000 bis ca. 1.500", None)]
    #[case("€ 100 a pezzo", None)]
    #[case("€ 1.000 total 1.500", None)]
    // Layouts a single price owns.
    #[case("12 € 50", None)]
    #[case("€ 100 - 20%", None)]
    // One amount only.
    #[case("ab 1.200 €", None)]
    #[case("from £1,200", None)]
    fn should_read_no_range_when_the_text_states_none(
        #[case] raw: &str,
        #[case] fallback: Option<Currency>,
    ) {
        assert_eq!(Ok(None), display_range(raw, fallback), "{raw}");
    }

    #[rstest]
    #[case("€ 1.000 - 1.500 CHF")]
    #[case("€ 1.000 - $ 1.500")]
    fn should_fail_closed_when_the_bounds_mix_currencies_the_fallback_does_not_name(
        #[case] raw: &str,
    ) {
        for fallback in [None, Some(Currency::Gbp)] {
            assert_eq!(
                Err(PriceError::InvalidRange),
                display_range(raw, fallback),
                "{raw}"
            );
        }
    }

    #[rstest]
    // The lower bound is not below the upper bound.
    #[case("€ 1.500 - 1.000")]
    #[case("€ 1.000 - 1.000")]
    // Several different ranges.
    #[case("€ 100 - 200, € 300 - 400")]
    #[case("€ 100 - 200 - 300")]
    // A range next to another price.
    #[case("Was € 2.000 Now € 1.000 - 1.500")]
    #[case("€ 1.000 – 1.500 € 2.000")]
    #[case("€ 1.000 - 1.500 zzgl. € 50 Versand")]
    // A dual-currency display of two ranges.
    #[case("€ 1.000 – 1.500 CHF 950 – 1.400")]
    fn should_fail_closed_when_the_range_layout_is_no_valid_range(#[case] raw: &str) {
        for fallback in [None, Some(Currency::Eur)] {
            assert_eq!(
                Err(PriceError::InvalidRange),
                display_range(raw, fallback),
                "{raw}"
            );
        }
    }

    #[rstest]
    #[case("€ 1.000 -1.500", PriceError::NegativeAmount)]
    #[case("400,00 -1.650,00 €", PriceError::NegativeAmount)]
    #[case("€ -100 - 200", PriceError::NegativeAmount)]
    #[case("€ 1.000 - −1.500", PriceError::NegativeAmount)]
    #[case("€ 1.000 – -1.500", PriceError::NegativeAmount)]
    #[case("€ 1,234,56 - 2.000", PriceError::ParseFailure)]
    #[case("€ 1 - 1,5 Mio.", PriceError::ParseFailure)]
    #[case("€ 1.000 - 92.233.720.368.547.758,08", PriceError::ParseFailure)]
    #[case("EUR 1.2345 - 2", PriceError::ExcessPrecision)]
    fn should_fail_closed_when_a_bound_is_no_amount(
        #[case] raw: &str,
        #[case] expected: PriceError,
    ) {
        assert_eq!(Err(expected), display_range(raw, None), "{raw}");
    }

    #[test]
    fn should_read_the_largest_storable_upper_bound() {
        assert_eq!(
            Ok(Some(DisplayRange {
                range: range(Currency::Eur, 0, i64::MAX as u64),
                explicit_currency: true,
            })),
            display_range("€ 0 - 92.233.720.368.547.758,07", None)
        );
    }

    #[test]
    fn should_read_one_range_when_a_label_repeats_it() {
        let raw = "€ 1 - 2 ".repeat(32);
        assert_eq!(
            Ok(Some(DisplayRange {
                range: range(Currency::Eur, 100, 200),
                explicit_currency: true,
            })),
            display_range(&raw, None)
        );
    }
}
