use super::error::NormalizationError;
use time::{OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description};

/// Attempts to parse a datetime string using a series of well-known formats.
///
/// Supported formats (in order of attempt):
/// 1. RFC 3339 / ISO 8601 with offset     (`2024-06-01T10:00:00+02:00`)
/// 2. ISO 8601-like with space separator  (`2024-06-01 10:00:00+02:00`)
/// 3. ISO 8601 date-only                  (`2024-06-01`)              → midnight UTC
/// 4. SQL datetime with seconds           (`2024-06-01 10:30:00`)     → UTC
/// 5. SQL datetime without seconds        (`2024-06-01 10:30`)        → UTC
/// 6. German datetime with seconds        (`01.06.2024 10:30:00`)     → UTC
/// 7. German datetime without seconds     (`01.06.2024 10:30`)        → UTC
/// 8. German date-only                    (`01.06.2024`)              → midnight UTC
/// 9. US datetime with seconds            (`06/01/2024 10:30:00`)     → UTC
/// 10. US datetime without seconds        (`06/01/2024 10:30`)        → UTC
/// 11. US date-only                       (`06/01/2024`)              → midnight UTC
/// 12. Unix epoch                         (`1717228800`)
pub(super) fn parse_datetime(raw: &str) -> Option<OffsetDateTime> {
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

/// Parses an optional raw datetime string into an optional [`OffsetDateTime`].
///
/// - `None` input → `Ok(None)`
/// - blank string → `Ok(None)`
/// - unparseable string → `Err(make_err(raw))`
pub(super) fn normalize_datetime_field(
    raw: Option<String>,
    make_err: impl Fn(String) -> NormalizationError,
) -> Result<Option<OffsetDateTime>, NormalizationError> {
    let Some(s) = raw else { return Ok(None) };

    let trimmed = s.trim().to_owned();
    if trimmed.is_empty() {
        return Ok(None);
    }

    parse_datetime(&trimmed)
        .map(Some)
        .ok_or_else(|| make_err(trimmed))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::OffsetDateTime;
    use time::macros::datetime;

    use super::{normalize_datetime_field, parse_datetime};
    use crate::normalization::error::NormalizationError;

    // -----------------------------------------------------------------------
    // RFC 3339 / ISO 8601 with offset
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01T10:00:00+02:00", datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-06-01T10:00:00Z", datetime!(2024-06-01 10:00:00 UTC))]
    #[case("2024-06-01T00:00:00Z", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-12-31T23:59:59Z", datetime!(2024-12-31 23:59:59 UTC))]
    #[case("2024-06-01T10:00:00-05:00", datetime!(2024-06-01 10:00:00 -5))]
    fn should_parse_datetime_when_rfc3339_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // ISO 8601-like with space instead of T
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01 10:00:00+02:00", datetime!(2024-06-01 10:00:00 +2))]
    #[case("2024-06-01 10:00:00Z", datetime!(2024-06-01 10:00:00 UTC))]
    fn should_parse_datetime_when_space_separated_iso8601_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // ISO 8601 date-only
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-12-31", datetime!(2024-12-31 00:00:00 UTC))]
    #[case("2000-01-01", datetime!(2000-01-01 00:00:00 UTC))]
    fn should_parse_datetime_when_iso_date_only_string_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // SQL-style (YYYY-MM-DD HH:MM[:SS], no timezone → UTC)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("2024-06-01 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("2024-06-01 00:00:00", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("2024-12-31 23:59:59", datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_sql_datetime_with_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("2024-06-01 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("2024-12-31 23:59", datetime!(2024-12-31 23:59:00 UTC))]
    fn should_parse_datetime_when_sql_datetime_without_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // German / European format (DD.MM.YYYY)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("01.06.2024 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("31.12.2024 23:59:59", datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_german_datetime_with_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("01.06.2024 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("31.12.2024 23:59", datetime!(2024-12-31 23:59:00 UTC))]
    fn should_parse_datetime_when_german_datetime_without_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("01.06.2024", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("31.12.2024", datetime!(2024-12-31 00:00:00 UTC))]
    fn should_parse_datetime_when_german_date_only_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // US format (MM/DD/YYYY)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("06/01/2024 10:30:00", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("12/31/2024 23:59:59", datetime!(2024-12-31 23:59:59 UTC))]
    fn should_parse_datetime_when_us_datetime_with_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("06/01/2024 10:30", datetime!(2024-06-01 10:30:00 UTC))]
    #[case("12/31/2024 23:59", datetime!(2024-12-31 23:59:00 UTC))]
    fn should_parse_datetime_when_us_datetime_without_seconds_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    #[rstest]
    #[case("06/01/2024", datetime!(2024-06-01 00:00:00 UTC))]
    #[case("12/31/2024", datetime!(2024-12-31 00:00:00 UTC))]
    fn should_parse_datetime_when_us_date_only_provided(
        #[case] raw: &str,
        #[case] expected: OffsetDateTime,
    ) {
        assert_eq!(parse_datetime(raw), Some(expected));
    }

    // -----------------------------------------------------------------------
    // Unix epoch
    // -----------------------------------------------------------------------

    #[test]
    fn should_parse_datetime_when_unix_epoch_string_provided() {
        let result = parse_datetime("1717228800");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unix_timestamp(), 1717228800);
    }

    #[test]
    fn should_parse_datetime_when_zero_unix_epoch_provided() {
        let result = parse_datetime("0");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unix_timestamp(), 0);
    }

    #[test]
    fn should_parse_datetime_when_negative_unix_epoch_provided() {
        // Timestamps before 1970 are valid.
        let result = parse_datetime("-86400");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unix_timestamp(), -86400);
    }

    // -----------------------------------------------------------------------
    // Whitespace handling
    // -----------------------------------------------------------------------

    #[test]
    fn should_trim_whitespace_before_parsing() {
        assert_eq!(
            parse_datetime("  2024-06-01T10:00:00Z  "),
            Some(datetime!(2024-06-01 10:00:00 UTC))
        );
    }

    // -----------------------------------------------------------------------
    // Unparseable inputs
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("not a date")]
    #[case("32.13.2024")]
    #[case("2024-13-01")]
    #[case("yesterday at noon")]
    #[case("next tuesday")]
    fn should_return_none_when_datetime_string_is_unparseable(#[case] raw: &str) {
        assert_eq!(parse_datetime(raw), None, "expected None for '{}'", raw);
    }

    #[test]
    fn should_return_none_when_input_is_empty() {
        assert_eq!(parse_datetime(""), None);
    }

    // -----------------------------------------------------------------------
    // normalize_datetime_field
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_none_when_raw_is_none_for_normalize_datetime_field() {
        let result = normalize_datetime_field(None, |r| {
            NormalizationError::AuctionStartParseError { raw: r }
        });
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn should_return_none_when_raw_is_blank_for_normalize_datetime_field() {
        let result = normalize_datetime_field(Some("  ".into()), |r| {
            NormalizationError::AuctionStartParseError { raw: r }
        });
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn should_return_parsed_datetime_when_valid_string_for_normalize_datetime_field() {
        let result = normalize_datetime_field(Some("2024-06-01T10:00:00Z".into()), |r| {
            NormalizationError::AuctionStartParseError { raw: r }
        });
        assert_eq!(result.unwrap(), Some(datetime!(2024-06-01 10:00:00 UTC)));
    }

    #[test]
    fn should_return_error_when_unparseable_string_for_normalize_datetime_field() {
        let result = normalize_datetime_field(Some("not a date".into()), |r| {
            NormalizationError::AuctionStartParseError { raw: r }
        });
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::AuctionStartParseError { raw } if raw == "not a date"
        ));
    }

    #[test]
    fn should_use_provided_error_constructor_when_field_is_auction_end() {
        let result = normalize_datetime_field(Some("bad".into()), |r| {
            NormalizationError::AuctionEndParseError { raw: r }
        });
        assert!(matches!(
            result.unwrap_err(),
            NormalizationError::AuctionEndParseError { .. }
        ));
    }
}
