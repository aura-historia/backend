//! Display-text numbers: the digit runs a price text holds and how one reads as an amount.

use super::PriceError;
use money::{Currency, HasMinorUnitExponent, MonetaryAmount};

pub(super) fn parse_price_number(
    number: &str,
    currency: &Currency,
) -> Result<MonetaryAmount, PriceError> {
    let exponent = minor_unit_exponent(currency);

    // Remove whitespace and apostrophes used as thousands separators.
    let stripped = number.replace([' ', '\u{00a0}', '\u{202f}', '\'', '_'], "");

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

    let fraction_normalised = normalise_fraction(&fraction_clean, exponent)?;

    let amount_str = format!("{}{}", integer_clean, fraction_normalised);
    let amount: u64 = amount_str.parse().map_err(|_| PriceError::ParseFailure)?;

    Ok(MonetaryAmount::from(amount))
}

/// Returns the number of minor-unit digits for the given currency (always 2
/// for the currencies we support).
pub(super) fn minor_unit_exponent(currency: &Currency) -> u32 {
    currency.minor_unit_exponent().0 as u32
}

/// Pads with trailing zeros to exactly `exponent` digits. Excess digits are accepted only when
/// they are zero padding; nonzero excess precision is never truncated.
fn normalise_fraction(frac: &str, exponent: u32) -> Result<String, PriceError> {
    let exp = exponent as usize;
    if frac.len() >= exp {
        if frac[exp..].bytes().any(|digit| digit != b'0') {
            return Err(PriceError::ExcessPrecision);
        }
        Ok(frac[..exp].to_owned())
    } else {
        Ok(format!("{:0<width$}", frac, width = exp))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PriceNumberCandidate {
    pub(super) value: String,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PriceNumberCandidateQuality {
    Clean,
    Malformed,
}

pub(super) fn extract_price_number_candidates(raw: &str) -> Vec<PriceNumberCandidate> {
    let mut candidates = Vec::new();
    let mut start = None;
    let mut last_digit_end = 0usize;

    for (idx, ch) in raw.char_indices() {
        if ch.is_ascii_digit() {
            if start.is_none() {
                start = Some(idx);
            }
            last_digit_end = idx + ch.len_utf8();
        } else if start.is_some()
            && matches!(ch, '.' | ',' | '\'' | '_' | ' ' | '\u{00a0}' | '\u{202f}')
        {
            continue;
        } else if let Some(s) = start.take() {
            candidates.push(price_number_candidate(raw, s, last_digit_end));
        }
    }

    if let Some(s) = start {
        candidates.push(price_number_candidate(raw, s, last_digit_end));
    }

    candidates
}

fn price_number_candidate(raw: &str, start: usize, end: usize) -> PriceNumberCandidate {
    let trimmed = raw[start..end].trim();
    let leading_trimmed = raw[start..end].len() - raw[start..end].trim_start().len();
    let trailing_trimmed = raw[start..end].len() - raw[start..end].trim_end().len();

    PriceNumberCandidate {
        value: trimmed.to_string(),
        start: start + leading_trimmed,
        end: end - trailing_trimmed,
    }
}

pub(super) fn price_number_candidate_quality(value: &str) -> PriceNumberCandidateQuality {
    let stripped = value.replace([' ', '\u{00a0}', '\u{202f}', '\'', '_'], "");
    let last_dot = stripped.rfind('.');
    let last_comma = stripped.rfind(',');
    let decimal_index = match (last_dot, last_comma) {
        (Some(dot), Some(comma)) => Some(dot.max(comma)),
        (Some(dot), None) => Some(dot),
        (None, Some(comma)) => Some(comma),
        (None, None) => None,
    };

    let Some(index) = decimal_index else {
        return PriceNumberCandidateQuality::Clean;
    };

    let trailing_digits = stripped[index + 1..]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .count();

    if trailing_digits > 3 {
        PriceNumberCandidateQuality::Malformed
    } else {
        PriceNumberCandidateQuality::Clean
    }
}

/// Returns `true` when the string slice is exactly 3 ASCII digits, which
/// indicates a thousands-separator group rather than a decimal fraction.
fn is_thousands_group(s: &str) -> bool {
    s.len() == 3 && s.chars().all(|c| c.is_ascii_digit())
}

/// Splits a cleaned number string at the decimal separator.
///
/// The *later* of the last `.` and last `,` is treated as the decimal
/// separator; the earlier one (if present) is the thousands separator.
///
/// Special case: when only a single separator is present and the trailing
/// group has **exactly 3 digits** it is treated as a thousands separator and
/// `None` is returned (e.g. `"8,800"` → `None`, `"8.800"` → `None`).
fn split_decimal(s: &str) -> Option<(&str, &str)> {
    let last_dot = s.rfind('.');
    let last_comma = s.rfind(',');

    match (last_dot, last_comma) {
        (None, None) => None,
        (Some(d), None) => {
            let after = &s[d + 1..];
            if is_thousands_group(after) {
                None
            } else {
                Some((&s[..d], after))
            }
        }
        (None, Some(c)) => {
            let after = &s[c + 1..];
            if is_thousands_group(after) {
                None
            } else {
                Some((&s[..c], after))
            }
        }
        (Some(d), Some(c)) => {
            if d > c {
                Some((&s[..d], &s[d + 1..]))
            } else {
                Some((&s[..c], &s[c + 1..]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{PriceError, normalise_fraction, split_decimal};

    #[rstest]
    #[case("1234", None)]
    #[case("1234.56", Some(("1234", "56")))]
    #[case("1234,56", Some(("1234", "56")))]
    // dot is later → dot is decimal separator
    #[case("1.234,56", Some(("1.234", "56")))]
    // comma is later → comma is decimal separator
    #[case("1,234.56", Some(("1,234", "56")))]
    // lone comma/dot followed by exactly 3 digits → thousands separator, not decimal
    #[case("8,800", None)]
    #[case("1,234", None)]
    #[case("8.800", None)]
    #[case("1.234", None)]
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
    #[case("56", 2, Ok("56"))]
    #[case("5", 2, Ok("50"))]
    #[case("", 2, Ok("00"))]
    #[case("120", 2, Ok("12"))]
    #[case("0", 2, Ok("00"))]
    #[case("123", 2, Err(PriceError::ExcessPrecision))]
    #[case("5", 0, Err(PriceError::ExcessPrecision))]
    fn should_normalise_fraction_without_truncating_nonzero_precision(
        #[case] frac: &str,
        #[case] exponent: u32,
        #[case] expected: Result<&str, PriceError>,
    ) {
        assert_eq!(
            normalise_fraction(frac, exponent),
            expected.map(str::to_owned)
        );
    }
}
