//! Display-text numbers: the digit runs a price text holds and how one reads as an amount.
//!
//! A display amount is one written number. Its thousands are grouped consistently or not at
//! all, its decimal separator is not its grouping separator, and nothing next to it continues
//! the number in another form, such as a percent sign, a fraction, a scale word or a digit of
//! another script. A digit run that reads as an amount only by joining numbers a display shows
//! side by side, such as `€ 12 50` read as `1250`, is no amount: it fails closed.

use super::PriceError;
use money::{Currency, HasMinorUnitExponent, MonetaryAmount};
use regex::regex;

/// The largest amount, in minor units, a price can hold. Prices persist as signed 64-bit minor
/// units, so a larger amount is rejected as invalid input here rather than failing the write.
pub(super) const MAX_MINOR_UNITS: u64 = i64::MAX as u64;

/// Reads one written number as an amount in `currency`'s minor units.
pub(super) fn parse_price_number(
    number: &str,
    currency: &Currency,
) -> Result<MonetaryAmount, PriceError> {
    let written = WrittenNumber::parse(number).ok_or(PriceError::ParseFailure)?;
    let (integer_groups, fraction) = written.split_decimal()?;
    if !is_grouped_consistently(&integer_groups, currency) {
        return Err(PriceError::ParseFailure);
    }

    let integer: String = integer_groups.groups.concat();
    let fraction = normalise_fraction(fraction.unwrap_or(""), minor_unit_exponent(currency))?;
    format!("{integer}{fraction}")
        .parse::<u64>()
        .ok()
        .filter(|amount| *amount <= MAX_MINOR_UNITS)
        .map(MonetaryAmount::from)
        .ok_or(PriceError::ParseFailure)
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

/// The separators a written number joins its digit groups with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Separator {
    Dot,
    Comma,
    /// `'` or `’`, as Swiss prices write `1'250` and `1’250`.
    Apostrophe,
    /// A space, no-break, narrow no-break or thin space.
    Space,
    Underscore,
}

impl Separator {
    fn of(ch: char) -> Option<Self> {
        match ch {
            '.' => Some(Self::Dot),
            ',' => Some(Self::Comma),
            '\'' | '\u{2019}' => Some(Self::Apostrophe),
            ' ' | '\u{00a0}' | '\u{202f}' | '\u{2009}' => Some(Self::Space),
            '_' => Some(Self::Underscore),
            _ => None,
        }
    }
}

fn is_group_space(ch: char) -> bool {
    Separator::of(ch) == Some(Separator::Space)
}

/// The ASCII digit groups of a written number and the one separator between each pair.
#[derive(Debug, PartialEq)]
struct WrittenNumber<'a> {
    groups: Vec<&'a str>,
    separators: Vec<Separator>,
}

impl<'a> WrittenNumber<'a> {
    /// Splits `number` into digit groups. Anything but digit groups joined by single separators,
    /// such as a leading, trailing or doubled separator, is no written number.
    fn parse(number: &'a str) -> Option<Self> {
        let mut groups = Vec::new();
        let mut separators = Vec::new();
        let mut rest = number;
        loop {
            let digits = rest
                .find(|ch: char| !ch.is_ascii_digit())
                .unwrap_or(rest.len());
            if digits == 0 {
                return None;
            }
            groups.push(&rest[..digits]);
            rest = &rest[digits..];
            let mut chars = rest.chars();
            match chars.next() {
                None => return Some(Self { groups, separators }),
                Some(ch) => separators.push(Separator::of(ch)?),
            }
            rest = chars.as_str();
        }
    }

    /// Splits off the decimals. The later of a dot and a comma separates them; a dot or a comma
    /// that is the only kind and is followed by exactly three digits groups thousands instead
    /// (`8,800`, `1.234.567`).
    fn split_decimal(self) -> Result<(Self, Option<&'a str>), PriceError> {
        let Some(&last) = self.separators.last() else {
            return Ok((self, None));
        };
        let other_decimal_kind = match last {
            Separator::Dot => Separator::Comma,
            Separator::Comma => Separator::Dot,
            _ => return Ok((self, None)),
        };
        let last_group = self.groups[self.groups.len() - 1];
        if last_group.len() == 3 && !self.separators.contains(&other_decimal_kind) {
            return Ok((self, None));
        }

        let Self {
            mut groups,
            mut separators,
        } = self;
        groups.pop();
        separators.pop();
        // A decimal separator also grouping the thousands (`1,234,56`) is ambiguous.
        if separators.contains(&last) {
            return Err(PriceError::ParseFailure);
        }
        Ok((Self { groups, separators }, Some(last_group)))
    }
}

/// Whether the whole-unit groups are grouped by one separator as thousands are written: up to
/// three digits that do not start with zero, then groups of three. Indian rupee prices group
/// the lakh and crore by two (`1,23,456`).
fn is_grouped_consistently(integer: &WrittenNumber<'_>, currency: &Currency) -> bool {
    let Some((first, rest)) = integer.groups.split_first() else {
        return false;
    };
    let Some((&separator, others)) = integer.separators.split_first() else {
        return true;
    };
    if others.iter().any(|other| *other != separator) || first.starts_with('0') {
        return false;
    }
    let thousands = first.len() <= 3 && rest.iter().all(|group| group.len() == 3);
    let lakh = *currency == Currency::Inr
        && first.len() <= 2
        && rest.split_last().is_some_and(|(last, middle)| {
            last.len() == 3 && middle.iter().all(|group| group.len() == 2)
        });
    thousands || lakh
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

/// The numbers in `raw` that can be an amount, each a run of digits and the separators between
/// them. A percentage is never one (see [`DigitRun`]).
pub(super) fn extract_price_number_candidates(raw: &str) -> Vec<PriceNumberCandidate> {
    digit_runs(raw)
        .into_iter()
        .filter_map(|run| run.amount)
        .map(|(start, end)| PriceNumberCandidate {
            value: raw[start..end].to_owned(),
            start,
            end,
        })
        .collect()
}

/// Where the percentages in `raw` start.
pub(super) fn percentage_starts(raw: &str) -> Vec<usize> {
    digit_runs(raw)
        .into_iter()
        .filter_map(|run| run.percentage_start)
        .collect()
}

/// A run of digits and number separators, starting and ending with a digit, read as an amount
/// and a percentage. A number a percent sign follows is a percentage, never an amount: `30% off`
/// and `inkl. 19 % MwSt.` hold none. When a space joins the percentage to a number before it, as
/// `$370 30% off` writes a discount badge next to the price, the number before that space can
/// still be one.
struct DigitRun {
    amount: Option<(usize, usize)>,
    percentage_start: Option<usize>,
}

impl DigitRun {
    fn read(raw: &str, start: usize, end: usize) -> Self {
        let percent = raw[end..]
            .trim_start_matches(|ch: char| ch.is_whitespace())
            .starts_with(['%', '\u{ff05}', '\u{2030}']);
        if !percent {
            return Self {
                amount: Some((start, end)),
                percentage_start: None,
            };
        }
        let run = &raw[start..end];
        let Some(space) = run.rfind(is_group_space) else {
            return Self {
                amount: None,
                percentage_start: Some(start),
            };
        };
        let percentage_offset = space
            + run[space..]
                .find(|ch: char| ch.is_ascii_digit())
                .unwrap_or_default();
        let amount_end = run[..space]
            .rfind(|ch: char| ch.is_ascii_digit())
            .map(|last_digit| start + last_digit + 1);
        Self {
            amount: amount_end.map(|end| (start, end)),
            percentage_start: Some(start + percentage_offset),
        }
    }
}

fn digit_runs(raw: &str) -> Vec<DigitRun> {
    let mut runs = Vec::new();
    let mut start = None;
    let mut last_digit_end = 0usize;

    for (idx, ch) in raw.char_indices() {
        if ch.is_ascii_digit() {
            start.get_or_insert(idx);
            last_digit_end = idx + ch.len_utf8();
        } else if start.is_some() && Separator::of(ch).is_some() {
            continue;
        } else if let Some(start) = start.take() {
            runs.push(DigitRun::read(raw, start, last_digit_end));
        }
    }
    if let Some(start) = start {
        runs.push(DigitRun::read(raw, start, last_digit_end));
    }
    runs
}

/// Whether the text around a candidate leaves it one amount rather than part of a longer
/// number form: a fraction or ratio (`1/2`), an exponent (`1e5`), a scale word (`1,5 Mio.`,
/// `$1.5M`, `JPY 1万`), a digit of another script (`12３`, `1½`, `100²`), or a decimal separator
/// with no whole units before it (`,50 €`). An abbreviation dot (`Fr.1'250`) is no separator.
pub(super) fn stands_alone(raw: &str, candidate: &PriceNumberCandidate) -> bool {
    !(continues_before(&raw[..candidate.start]) || continues_after(&raw[candidate.end..]))
}

fn continues_before(before: &str) -> bool {
    let mut chars = before.chars().rev();
    match chars.next() {
        Some(ch) if is_other_script_digit(ch) => true,
        Some('.' | ',') => chars.next().is_none_or(|ch| !ch.is_alphabetic()),
        Some('/') => chars.next().is_some_and(is_any_digit),
        _ => false,
    }
}

fn continues_after(after: &str) -> bool {
    let mut chars = after.chars();
    match chars.next() {
        Some(ch) if is_other_script_digit(ch) => true,
        Some('/') => chars.next().is_some_and(is_any_digit),
        Some('e' | 'E') if regex!(r"^[eE][+\-]?[0-9]").is_match(after) => true,
        _ => regex!(
            r"^[ \u{a0}\u{202f}\u{2009}]?(?:(?i:k|m|mn|bn|mio|mrd|tsd|tausend|thousand|millions?|millionen|milliarden?|milliards?|billions?|lakhs?|crores?|mil|mila|mille|milion[ei]?|millón|millones)(?:[^\p{L}]|$)|[万萬千億亿만천억])"
        )
        .is_match(after),
    }
}

fn is_other_script_digit(ch: char) -> bool {
    ch.is_numeric() && !ch.is_ascii_digit()
}

fn is_any_digit(ch: char) -> bool {
    ch.is_numeric()
}

pub(super) fn price_number_candidate_quality(value: &str) -> PriceNumberCandidateQuality {
    let last_dot = value.rfind('.');
    let last_comma = value.rfind(',');
    let decimal_index = match (last_dot, last_comma) {
        (Some(dot), Some(comma)) => Some(dot.max(comma)),
        (Some(dot), None) => Some(dot),
        (None, Some(comma)) => Some(comma),
        (None, None) => None,
    };

    let Some(index) = decimal_index else {
        return PriceNumberCandidateQuality::Clean;
    };

    let trailing_digits = value[index + 1..]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .count();

    if trailing_digits > 3 {
        PriceNumberCandidateQuality::Malformed
    } else {
        PriceNumberCandidateQuality::Clean
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use money::{Currency, MonetaryAmount};

    use super::{
        MAX_MINOR_UNITS, PriceError, Separator, WrittenNumber, extract_price_number_candidates,
        normalise_fraction, parse_price_number, stands_alone,
    };

    #[rstest]
    #[case("1234", vec!["1234"], None)]
    #[case("1234.56", vec!["1234"], Some("56"))]
    #[case("1234,56", vec!["1234"], Some("56"))]
    // dot is later → dot is decimal separator
    #[case("1.234,56", vec!["1", "234"], Some("56"))]
    // comma is later → comma is decimal separator
    #[case("1,234.56", vec!["1", "234"], Some("56"))]
    // lone comma/dot followed by exactly 3 digits → thousands separator, not decimal
    #[case("8,800", vec!["8", "800"], None)]
    #[case("1,234", vec!["1", "234"], None)]
    #[case("8.800", vec!["8", "800"], None)]
    #[case("1.234", vec!["1", "234"], None)]
    #[case("1.234.567", vec!["1", "234", "567"], None)]
    #[case("1 234,5", vec!["1", "234"], Some("5"))]
    fn should_split_decimal_correctly_for_various_formats(
        #[case] input: &str,
        #[case] integer: Vec<&str>,
        #[case] fraction: Option<&str>,
    ) {
        let (integer_part, fraction_part) = WrittenNumber::parse(input)
            .unwrap_or_else(|| panic!("{input} is a written number"))
            .split_decimal()
            .unwrap_or_else(|error| panic!("{input} splits: {error:?}"));
        assert_eq!(integer, integer_part.groups);
        assert_eq!(fraction, fraction_part);
    }

    #[rstest]
    #[case("1,234,56")]
    #[case("12.34.56")]
    #[case("1.2.3")]
    fn should_reject_a_decimal_separator_that_also_groups_thousands(#[case] input: &str) {
        let written = WrittenNumber::parse(input).unwrap_or_else(|| panic!("{input}"));
        assert_eq!(Err(PriceError::ParseFailure), written.split_decimal());
    }

    #[rstest]
    #[case("1..2")]
    #[case("1,,234")]
    #[case("12, 50")]
    #[case(".50")]
    #[case("50.")]
    #[case("1-2")]
    #[case("")]
    fn should_not_read_doubled_dangling_or_foreign_separators_as_one_number(#[case] input: &str) {
        assert_eq!(None, WrittenNumber::parse(input));
    }

    #[test]
    fn should_read_every_grouping_separator() {
        for (input, separator) in [
            ("1.250", Separator::Dot),
            ("1,250", Separator::Comma),
            ("1'250", Separator::Apostrophe),
            ("1\u{2019}250", Separator::Apostrophe),
            ("1 250", Separator::Space),
            ("1\u{00a0}250", Separator::Space),
            ("1\u{202f}250", Separator::Space),
            ("1\u{2009}250", Separator::Space),
            ("1_250", Separator::Underscore),
        ] {
            assert_eq!(
                Some(WrittenNumber {
                    groups: vec!["1", "250"],
                    separators: vec![separator],
                }),
                WrittenNumber::parse(input),
                "{input}"
            );
            assert_eq!(
                Ok(MonetaryAmount::from(125_000_u64)),
                parse_price_number(input, &Currency::Chf),
                "{input}"
            );
        }
    }

    #[rstest]
    #[case("1.234.567,89", 123_456_789)]
    #[case("1,234,567.89", 123_456_789)]
    #[case("1 234 567,89", 123_456_789)]
    #[case("1'234'567.89", 123_456_789)]
    #[case("1\u{2019}250.00", 125_000)]
    #[case("12,500", 1_250_000)]
    #[case("00012", 1_200)]
    #[case("0.50", 50)]
    #[case("0", 0)]
    fn should_read_consistently_grouped_numbers(#[case] input: &str, #[case] expected: u64) {
        assert_eq!(
            Ok(MonetaryAmount::from(expected)),
            parse_price_number(input, &Currency::Eur),
            "{input}"
        );
    }

    #[rstest]
    // A space joins only a group of three digits to the number before it.
    #[case("12 50")]
    #[case("370 30")]
    #[case("1 2 3")]
    #[case("1 2500")]
    // Groups after the first have three digits, the first at most three and no leading zero.
    #[case("1,23,4.56")]
    #[case("1.23.456")]
    #[case("1250.000")]
    #[case("1234,567.89")]
    #[case("0,001")]
    #[case("0.500")]
    // One grouping separator only.
    #[case("1.234 567")]
    #[case("1,234.567,89")]
    #[case("1'234,567.89")]
    // A decimal separator that also groups thousands.
    #[case("1,234,56")]
    #[case("12.34.56")]
    fn should_reject_inconsistently_grouped_numbers(#[case] input: &str) {
        assert_eq!(
            Err(PriceError::ParseFailure),
            parse_price_number(input, &Currency::Eur),
            "{input}"
        );
    }

    #[rstest]
    #[case("1,23,456", 12_345_600)]
    #[case("12,34,567.50", 123_456_750)]
    #[case("1,23,45,678", 1_234_567_800)]
    fn should_read_lakh_grouping_for_indian_rupees_only(
        #[case] input: &str,
        #[case] expected: u64,
    ) {
        assert_eq!(
            Ok(MonetaryAmount::from(expected)),
            parse_price_number(input, &Currency::Inr)
        );
        assert_eq!(
            Err(PriceError::ParseFailure),
            parse_price_number(input, &Currency::Gbp)
        );
    }

    #[test]
    fn should_reject_amounts_beyond_the_storable_minor_units() {
        assert_eq!(
            Ok(MonetaryAmount::from(MAX_MINOR_UNITS)),
            parse_price_number("92233720368547758.07", &Currency::Eur)
        );
        assert_eq!(
            Err(PriceError::ParseFailure),
            parse_price_number("92233720368547758.08", &Currency::Eur)
        );
        assert_eq!(
            Err(PriceError::ParseFailure),
            parse_price_number("99999999999999999999", &Currency::Eur)
        );
    }

    #[rstest]
    #[case("$370 30% off", vec!["370"])]
    #[case("$1 000 20% off", vec!["1 000"])]
    #[case("€100 2,5 % Rabatt", vec!["100"])]
    #[case("100 € inkl. 19 % MwSt.", vec!["100"])]
    #[case("30% -> 23,800원", vec!["23,800"])]
    #[case("$37030% off", vec![])]
    #[case("30\u{ff05}", vec![])]
    #[case("5\u{2030}", vec![])]
    fn should_never_take_a_percentage_for_an_amount(
        #[case] raw: &str,
        #[case] expected: Vec<&str>,
    ) {
        let values: Vec<_> = extract_price_number_candidates(raw)
            .into_iter()
            .map(|candidate| candidate.value)
            .collect();
        assert_eq!(expected, values, "{raw}");
    }

    #[rstest]
    #[case("€ 1/2")]
    #[case("€ 100/200")]
    #[case("€ 1e5")]
    #[case("€ 1.2E+3")]
    #[case("€ 1,5 Mio.")]
    #[case("$1.5M")]
    #[case("$1k")]
    #[case("£2.5 m")]
    #[case("€ 5 Tsd.")]
    #[case("$2 million")]
    #[case("JPY 1万")]
    #[case("€ 12\u{ff13}")]
    #[case("£1½")]
    #[case("€100²")]
    #[case(",50 €")]
    #[case("€.50")]
    fn should_not_read_a_number_that_continues_in_another_form(#[case] raw: &str) {
        let candidates = extract_price_number_candidates(raw);
        assert!(
            candidates
                .iter()
                .any(|candidate| !stands_alone(raw, candidate)),
            "{raw}: {candidates:?}"
        );
    }

    #[rstest]
    #[case("€100")]
    #[case("Fr.1'250.00")]
    #[case("Nr. 12 €100")]
    #[case("€100/Stück")]
    #[case("€ 100 kg")]
    #[case("€1.250,00inkl. MwSt.")]
    #[case("€ 100 Mahagoni")]
    #[case("700円")]
    #[case("€ 100 each")]
    fn should_read_a_number_that_ends_where_the_amount_ends(#[case] raw: &str) {
        let candidates = extract_price_number_candidates(raw);
        assert!(!candidates.is_empty(), "{raw}");
        assert!(
            candidates
                .iter()
                .all(|candidate| stands_alone(raw, candidate)),
            "{raw}: {candidates:?}"
        );
    }

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
