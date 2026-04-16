//! Keyword-based normalizers for product attribute enum fields:
//! [`Authenticity`], [`Condition`], [`Provenance`], [`Restoration`], and [`OriginYear`].
//!
//! Each function accepts an `Option<String>` (the raw text extracted by the CSS rule) and
//! returns the most appropriate enum variant, falling back to `Unknown` / `None` when the
//! raw text is absent or unrecognised.  The matching is case-insensitive and trims whitespace.

use common::year::{Year, YearRange};
use product::core::{
    authenticity::Authenticity, condition::Condition, origin_year::OriginYear,
    provenance::Provenance, restoration::Restoration,
};

// ---------------------------------------------------------------------------
// Authenticity
// ---------------------------------------------------------------------------

/// Map a raw extracted string to an [`Authenticity`] variant.
///
/// Returns [`Authenticity::Unknown`] when the input is `None` or unrecognised.
pub fn normalize_authenticity(raw: Option<String>) -> Authenticity {
    let Some(raw) = raw else {
        return Authenticity::Unknown;
    };
    let lower = raw.trim().to_lowercase();
    if lower.contains("original") || lower.contains("echt") || lower.contains("authentique") {
        Authenticity::Original
    } else if lower.contains("later copy")
        || lower.contains("spätere kopie")
        || lower.contains("copie ultérieure")
        || lower.contains("later copy")
    {
        Authenticity::LaterCopy
    } else if lower.contains("reproduction")
        || lower.contains("reproduktion")
        || lower.contains("replik")
        || lower.contains("replica")
        || lower.contains("réplique")
        || lower.contains("copia")
    {
        Authenticity::Reproduction
    } else if lower.contains("questionable")
        || lower.contains("zweifelhaft")
        || lower.contains("douteux")
        || lower.contains("discutible")
    {
        Authenticity::Questionable
    } else {
        Authenticity::Unknown
    }
}

// ---------------------------------------------------------------------------
// Condition
// ---------------------------------------------------------------------------

/// Map a raw extracted string to a [`Condition`] variant.
///
/// Returns [`Condition::Unknown`] when the input is `None` or unrecognised.
pub fn normalize_condition(raw: Option<String>) -> Condition {
    let Some(raw) = raw else {
        return Condition::Unknown;
    };
    let lower = raw.trim().to_lowercase();
    // Check most-specific strings first.
    if lower.contains("excellent") || lower.contains("ausgezeichnet") || lower.contains("excellent")
    {
        Condition::Excellent
    } else if lower.contains("very good")
        || lower.contains("sehr gut")
        || lower.contains("très bon")
    {
        Condition::Great
    } else if lower.contains("good")
        || lower.contains("gut")
        || lower.contains("bon")
        || lower.contains("buono")
    {
        Condition::Good
    } else if lower.contains("fair")
        || lower.contains("befriedigend")
        || lower.contains("passable")
        || lower.contains("discreto")
    {
        Condition::Fair
    } else if lower.contains("poor")
        || lower.contains("schlecht")
        || lower.contains("mauvais")
        || lower.contains("scarso")
    {
        Condition::Poor
    } else {
        Condition::Unknown
    }
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Map a raw extracted string to a [`Provenance`] variant.
///
/// Returns [`Provenance::Unknown`] when the input is `None` or unrecognised.
pub fn normalize_provenance(raw: Option<String>) -> Provenance {
    let Some(raw) = raw else {
        return Provenance::Unknown;
    };
    let lower = raw.trim().to_lowercase();
    if lower.contains("complete") || lower.contains("vollständig") || lower.contains("complet") {
        Provenance::Complete
    } else if lower.contains("partial")
        || lower.contains("teilweise")
        || lower.contains("partiel")
        || lower.contains("parziale")
    {
        Provenance::Partial
    } else if lower.contains("claimed")
        || lower.contains("behauptet")
        || lower.contains("revendiqué")
        || lower.contains("dichiarata")
    {
        Provenance::Claimed
    } else if lower.contains("no provenance")
        || lower.contains("keine provenienz")
        || lower.contains("sans provenance")
        || lower.contains("senza provenienza")
        || lower == "none"
    {
        Provenance::None
    } else {
        Provenance::Unknown
    }
}

// ---------------------------------------------------------------------------
// Restoration
// ---------------------------------------------------------------------------

/// Map a raw extracted string to a [`Restoration`] variant.
///
/// Returns [`Restoration::Unknown`] when the input is `None` or unrecognised.
pub fn normalize_restoration(raw: Option<String>) -> Restoration {
    let Some(raw) = raw else {
        return Restoration::Unknown;
    };
    let lower = raw.trim().to_lowercase();
    if lower.contains("major")
        || lower.contains("extensive")
        || lower.contains("stark restauriert")
        || lower.contains("restauration majeure")
    {
        Restoration::Major
    } else if lower.contains("minor")
        || lower.contains("leicht restauriert")
        || lower.contains("restauration mineure")
        || lower.contains("piccolo restauro")
    {
        Restoration::Minor
    } else if lower.contains("unrestored")
        || lower.contains("nicht restauriert")
        || lower.contains("non restauré")
        || lower.contains("non restaurato")
        || lower == "none"
        || lower.contains("no restoration")
    {
        Restoration::None
    } else {
        Restoration::Unknown
    }
}

// ---------------------------------------------------------------------------
// OriginYear
// ---------------------------------------------------------------------------

/// Parse a raw extracted string into an [`OriginYear`].
///
/// Recognises several common patterns:
/// - `"circa 1920"` / `"c. 1920"` / `"ca. 1920"` → `ExactYear(1920)`
/// - `"1850–1870"` / `"1850-1870"` / `"1850 to 1870"` → `EstimatedRange(1850..=1870)`
/// - `"18th century"` / `"18. Jahrhundert"` → `EstimatedRange(1700..=1799)`
/// - `"4-digit bare year"` → `ExactYear`
///
/// Returns `None` when the input is absent or no year could be parsed.
pub fn normalize_origin_year(raw: Option<String>) -> Option<OriginYear> {
    let raw = raw?.trim().to_string();
    if raw.is_empty() {
        return None;
    }

    // --- range pattern: YYYY–YYYY or YYYY-YYYY or YYYY to YYYY ----------------
    let range_re = regex::Regex::new(r"(\d{3,4})\s*[-–—to]+\s*(\d{3,4})").unwrap();
    if let Some(caps) = range_re.captures(&raw) {
        if let (Ok(start), Ok(end)) = (caps[1].parse::<i32>(), caps[2].parse::<i32>()) {
            let min = Some(Year::from(start));
            let max = Some(Year::from(end));
            return Some(OriginYear::EstimatedRange(YearRange { min, max }));
        }
    }

    // --- century pattern: "18th century", "18. Jahrhundert", "XVIIIe siècle" --
    let century_re = regex::Regex::new(
        r"(?i)\b(\d{1,2})(?:st|nd|rd|th)?\s*(?:century|jahrhundert|siècle|secolo|siglo)\b",
    )
    .unwrap();
    if let Some(caps) = century_re.captures(&raw) {
        if let Ok(century) = caps[1].parse::<i32>() {
            let start = (century - 1) * 100;
            let end = start + 99;
            let min = Some(Year::from(start));
            let max = Some(Year::from(end));
            return Some(OriginYear::EstimatedRange(YearRange { min, max }));
        }
    }

    // --- circa / exact year: "circa 1920", "c. 1920", "ca. 1920", bare "1920" -
    let circa_re = regex::Regex::new(r"(?i)(?:circa|c\.?|ca\.?)?\s*(\d{3,4})\b").unwrap();
    if let Some(caps) = circa_re.captures(&raw) {
        if let Ok(year) = caps[1].parse::<i32>() {
            return Some(OriginYear::ExactYear(Year::from(year)));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Authenticity --------------------------------------------------------

    #[test]
    fn authenticity_original() {
        assert_eq!(
            normalize_authenticity(Some("Original".into())),
            Authenticity::Original
        );
        assert_eq!(
            normalize_authenticity(Some("original work".into())),
            Authenticity::Original
        );
    }

    #[test]
    fn authenticity_reproduction() {
        assert_eq!(
            normalize_authenticity(Some("Reproduction".into())),
            Authenticity::Reproduction
        );
        assert_eq!(
            normalize_authenticity(Some("replica".into())),
            Authenticity::Reproduction
        );
    }

    #[test]
    fn authenticity_later_copy() {
        assert_eq!(
            normalize_authenticity(Some("Later copy".into())),
            Authenticity::LaterCopy
        );
    }

    #[test]
    fn authenticity_questionable() {
        assert_eq!(
            normalize_authenticity(Some("questionable authenticity".into())),
            Authenticity::Questionable
        );
    }

    #[test]
    fn authenticity_unknown_on_none() {
        assert_eq!(normalize_authenticity(None), Authenticity::Unknown);
    }

    #[test]
    fn authenticity_unknown_on_unrecognised() {
        assert_eq!(
            normalize_authenticity(Some("foobar".into())),
            Authenticity::Unknown
        );
    }

    // --- Condition -----------------------------------------------------------

    #[test]
    fn condition_excellent() {
        assert_eq!(
            normalize_condition(Some("Excellent condition".into())),
            Condition::Excellent
        );
    }

    #[test]
    fn condition_good() {
        assert_eq!(normalize_condition(Some("Good".into())), Condition::Good);
    }

    #[test]
    fn condition_very_good_maps_to_great() {
        assert_eq!(
            normalize_condition(Some("Very good".into())),
            Condition::Great
        );
    }

    #[test]
    fn condition_fair() {
        assert_eq!(
            normalize_condition(Some("fair condition".into())),
            Condition::Fair
        );
    }

    #[test]
    fn condition_poor() {
        assert_eq!(normalize_condition(Some("poor".into())), Condition::Poor);
    }

    #[test]
    fn condition_unknown_on_none() {
        assert_eq!(normalize_condition(None), Condition::Unknown);
    }

    // --- Provenance ----------------------------------------------------------

    #[test]
    fn provenance_complete() {
        assert_eq!(
            normalize_provenance(Some("Complete provenance".into())),
            Provenance::Complete
        );
    }

    #[test]
    fn provenance_partial() {
        assert_eq!(
            normalize_provenance(Some("Partial".into())),
            Provenance::Partial
        );
    }

    #[test]
    fn provenance_claimed() {
        assert_eq!(
            normalize_provenance(Some("Claimed provenance".into())),
            Provenance::Claimed
        );
    }

    #[test]
    fn provenance_none_keyword() {
        assert_eq!(
            normalize_provenance(Some("No provenance".into())),
            Provenance::None
        );
        assert_eq!(normalize_provenance(Some("none".into())), Provenance::None);
    }

    #[test]
    fn provenance_unknown_on_none() {
        assert_eq!(normalize_provenance(None), Provenance::Unknown);
    }

    // --- Restoration ---------------------------------------------------------

    #[test]
    fn restoration_none_keyword() {
        assert_eq!(
            normalize_restoration(Some("Unrestored".into())),
            Restoration::None
        );
        assert_eq!(
            normalize_restoration(Some("none".into())),
            Restoration::None
        );
        assert_eq!(
            normalize_restoration(Some("No restoration".into())),
            Restoration::None
        );
    }

    #[test]
    fn restoration_minor() {
        assert_eq!(
            normalize_restoration(Some("Minor restoration".into())),
            Restoration::Minor
        );
    }

    #[test]
    fn restoration_major() {
        assert_eq!(
            normalize_restoration(Some("Major restoration".into())),
            Restoration::Major
        );
    }

    #[test]
    fn restoration_unknown_on_none() {
        assert_eq!(normalize_restoration(None), Restoration::Unknown);
    }

    // --- OriginYear ----------------------------------------------------------

    #[test]
    fn origin_year_exact() {
        let result = normalize_origin_year(Some("1920".into()));
        assert!(matches!(result, Some(OriginYear::ExactYear(_))));
    }

    #[test]
    fn origin_year_circa() {
        let result = normalize_origin_year(Some("circa 1850".into()));
        assert!(matches!(result, Some(OriginYear::ExactYear(_))));
    }

    #[test]
    fn origin_year_range_dash() {
        let result = normalize_origin_year(Some("1850-1870".into()));
        assert!(matches!(result, Some(OriginYear::EstimatedRange(_))));
    }

    #[test]
    fn origin_year_range_en_dash() {
        let result = normalize_origin_year(Some("1850–1870".into()));
        assert!(matches!(result, Some(OriginYear::EstimatedRange(_))));
    }

    #[test]
    fn origin_year_century() {
        let result = normalize_origin_year(Some("18th century".into()));
        if let Some(OriginYear::EstimatedRange(r)) = result {
            let min: Option<i32> = r
                .min
                .and_then(|y| serde_json::from_value(serde_json::to_value(y).unwrap()).ok());
            let max: Option<i32> = r
                .max
                .and_then(|y| serde_json::from_value(serde_json::to_value(y).unwrap()).ok());
            assert_eq!(min, Some(1700));
            assert_eq!(max, Some(1799));
        } else {
            panic!("expected EstimatedRange, got {:?}", result);
        }
    }

    #[test]
    fn origin_year_none_on_none() {
        assert_eq!(normalize_origin_year(None), None);
    }

    #[test]
    fn origin_year_none_on_empty() {
        assert_eq!(normalize_origin_year(Some("".into())), None);
    }
}
