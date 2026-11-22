//! Currency markers in price text: symbols, ISO codes and currency words.

use money::Currency;
use regex::regex;
use strum::IntoEnumIterator;

pub(super) fn first_currency_marker_span(raw: &str) -> Option<(usize, usize)> {
    currency_marker_spans(raw).into_iter().next()
}

// Bare `¥` and `R` are ambiguous; `$` remains a USD marker for compatibility. `円` is only JPY
// and `원` only KRW. Symbols match anywhere, so currency words are matched separately as tokens.
pub(super) const CURRENCY_SYMBOLS: &[(Currency, &str)] = &[
    (Currency::Eur, "€"),
    (Currency::Gbp, "£"),
    (Currency::Usd, "$"),
    (Currency::Aud, "A$"),
    (Currency::Cad, "C$"),
    (Currency::Nzd, "NZ$"),
    (Currency::Cny, "CN¥"),
    (Currency::Brl, "R$"),
    (Currency::Pln, "zł"),
    (Currency::Try, "₺"),
    (Currency::Czk, "Kč"),
    (Currency::Rub, "₽"),
    (Currency::Aed, "د.إ"),
    (Currency::Sar, "﷼"),
    (Currency::Hkd, "HK$"),
    (Currency::Sgd, "S$"),
    (Currency::Krw, "₩"),
    (Currency::Inr, "₹"),
    (Currency::Twd, "NT$"),
    (Currency::Mxn, "MX$"),
    (Currency::Thb, "฿"),
    (Currency::Jpy, "円"),
    (Currency::Krw, "원"),
];

#[derive(Clone, Copy)]
pub(super) struct CurrencyMarker {
    pub(super) currency: Currency,
    pub(super) start: usize,
    pub(super) end: usize,
}

pub(super) fn currency_markers(raw: &str) -> Vec<CurrencyMarker> {
    let mut markers = Vec::new();

    for currency in Currency::iter() {
        markers.extend(
            raw.match_indices(currency.as_str())
                .filter(|(start, code)| is_currency_code_token(raw, *start, code.len()))
                .map(|(start, code)| CurrencyMarker {
                    currency,
                    start,
                    end: start + code.len(),
                }),
        );
    }

    for &(currency, symbol) in CURRENCY_SYMBOLS {
        markers.extend(
            raw.match_indices(symbol)
                .map(|(start, symbol)| CurrencyMarker {
                    currency,
                    start,
                    end: start + symbol.len(),
                }),
        );
    }

    // Currency names are words, so they need the same token boundaries as ISO codes: `30 euro`
    // and `30 Euros` are EUR, `Europe` is not.
    markers.extend(
        regex!(r"(?i)euros?")
            .find_iter(raw)
            .filter(|word| is_currency_code_token(raw, word.start(), word.len()))
            .map(|word| CurrencyMarker {
                currency: Currency::Eur,
                start: word.start(),
                end: word.end(),
            }),
    );

    markers.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.end.cmp(&left.end))
    });

    let mut non_overlapping_markers = Vec::new();
    for marker in markers {
        if non_overlapping_markers
            .last()
            .is_none_or(|previous: &CurrencyMarker| previous.end <= marker.start)
        {
            non_overlapping_markers.push(marker);
        }
    }
    non_overlapping_markers
}

fn is_currency_code_token(raw: &str, start: usize, code_len: usize) -> bool {
    let before = raw[..start].chars().next_back();
    let after = raw[start + code_len..].chars().next();

    before.is_none_or(is_currency_code_token_boundary)
        && after.is_none_or(is_currency_code_token_boundary)
}

fn is_currency_code_token_boundary(ch: char) -> bool {
    !ch.is_alphanumeric() && ch != '_'
}

pub(super) fn currency_marker_spans(raw: &str) -> Vec<(usize, usize)> {
    currency_markers(raw)
        .into_iter()
        .map(|marker| (marker.start, marker.end))
        .collect()
}
