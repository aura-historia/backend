use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::origin_year::OriginYear;
use crate::core::prohibited_content::ProhibitedContent;
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
use crate::service::product_command::CreateProductCommand;
use common::year::Year;
use once_cell::sync::Lazy;
use regex::Regex;

/// Extracts all searchable text from a product command by combining
/// the native title with the native description (if present).
fn extract_text(cmd: &CreateProductCommand) -> String {
    let title = cmd.native_title.payload.as_ref();
    match &cmd.native_description {
        Some(desc) => format!("{} {}", title, desc.payload.as_ref()),
        None => title.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// classify_images – ProhibitedContent::NaziGermany detection
// ---------------------------------------------------------------------------

/// High-confidence terms that strongly indicate Nazi-era imagery.
/// Every keyword is lowercase for case-insensitive matching.
const NAZI_KEYWORDS: &[&str] = &[
    // German
    "drittes reich",
    "dritten reich",
    "nationalsozialismus",
    "nationalsozialistisch",
    "hakenkreuz",
    "hakenkreuzfahne",
    "reichsadler",
    "hitlerjugend",
    "nsdap",
    "schutzstaffel",
    "sturmabteilung",
    "reichsführer",
    "reichsparteitag",
    // English
    "third reich",
    "national socialism",
    "nazi germany",
    "swastika",
    "hitler youth",
    // French
    "troisième reich",
    "croix gammée",
    "national-socialisme",
    "nationalsocialisme",
    "allemagne nazie",
    // Spanish
    "tercer reich",
    "esvástica",
    "cruz gamada",
    "nacionalsocialismo",
    "alemania nazi",
    // Italian
    "terzo reich",
    "svastica",
    "nazionalsocialismo",
    "germania nazista",
];

/// Returns `true` when the lowercased `text` contains the given `keyword`
/// surrounded by non-alphanumeric boundaries (or string edges), preventing
/// partial-word false positives (e.g. "nsdap" must not match "nsdapper").
fn contains_keyword(text: &str, keyword: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = text[start..].find(keyword) {
        let abs = start + pos;
        let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
        let end = abs + keyword.len();
        let after_ok = end == text.len() || !text.as_bytes()[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Analyses the product text and decides on a `ProhibitedContent` flag for
/// the images.
///
/// * If the text **clearly** suggests Nazi-related content →
///   `ProhibitedContent::NaziGermany` on every image.
/// * If no indicator is found → `ProhibitedContent::None`.
/// * `ProhibitedContent::Unknown` is **never** set by this function – it is
///   the default before any analysis runs.
pub fn classify_images(cmd: &mut CreateProductCommand) {
    let text = extract_text(cmd).to_lowercase();

    let is_nazi = NAZI_KEYWORDS
        .iter()
        .any(|keyword| contains_keyword(&text, keyword));

    let decision = if is_nazi {
        ProhibitedContent::NaziGermany
    } else {
        ProhibitedContent::None
    };

    for image in &mut cmd.images {
        image.prohibited_content = decision;
    }
}

// ---------------------------------------------------------------------------
// enrich_origin_year – extract a year from the product text
// ---------------------------------------------------------------------------

/// Matches a standalone four-digit year between 1000 and 2025.
static YEAR_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b(\d{4})\b").expect("year regex should compile"));

/// Century expressions mapped to a rough (min, max) year range.
/// These use the **ordinal** century number, e.g. "18th" → 1700-1799.
const CENTURY_PATTERNS: &[(&str, i32, i32)] = &[
    // English
    ("11th century", 1000, 1099),
    ("12th century", 1100, 1199),
    ("13th century", 1200, 1299),
    ("14th century", 1300, 1399),
    ("15th century", 1400, 1499),
    ("16th century", 1500, 1599),
    ("17th century", 1600, 1699),
    ("18th century", 1700, 1799),
    ("19th century", 1800, 1899),
    ("20th century", 1900, 1999),
    // German
    ("11. jahrhundert", 1000, 1099),
    ("12. jahrhundert", 1100, 1199),
    ("13. jahrhundert", 1200, 1299),
    ("14. jahrhundert", 1300, 1399),
    ("15. jahrhundert", 1400, 1499),
    ("16. jahrhundert", 1500, 1599),
    ("17. jahrhundert", 1600, 1699),
    ("18. jahrhundert", 1700, 1799),
    ("19. jahrhundert", 1800, 1899),
    ("20. jahrhundert", 1900, 1999),
    // French
    ("xie siècle", 1000, 1099),
    ("xiie siècle", 1100, 1199),
    ("xiiie siècle", 1200, 1299),
    ("xive siècle", 1300, 1399),
    ("xve siècle", 1400, 1499),
    ("xvie siècle", 1500, 1599),
    ("xviie siècle", 1600, 1699),
    ("xviiie siècle", 1700, 1799),
    ("xixe siècle", 1800, 1899),
    ("xxe siècle", 1900, 1999),
    // Spanish
    ("siglo xi", 1000, 1099),
    ("siglo xii", 1100, 1199),
    ("siglo xiii", 1200, 1299),
    ("siglo xiv", 1300, 1399),
    ("siglo xv", 1400, 1499),
    ("siglo xvi", 1500, 1599),
    ("siglo xvii", 1600, 1699),
    ("siglo xviii", 1700, 1799),
    ("siglo xix", 1800, 1899),
    ("siglo xx", 1900, 1999),
    // Italian
    ("xi secolo", 1000, 1099),
    ("xii secolo", 1100, 1199),
    ("xiii secolo", 1200, 1299),
    ("xiv secolo", 1300, 1399),
    ("xv secolo", 1400, 1499),
    ("xvi secolo", 1500, 1599),
    ("xvii secolo", 1600, 1699),
    ("xviii secolo", 1700, 1799),
    ("xix secolo", 1800, 1899),
    ("xx secolo", 1900, 1999),
];

/// Attempts to extract an origin year from the product text.
///
/// * Looks for a **standalone four-digit year** in the range 1000-2025.
///   When found, returns `OriginYear::ExactYear`.
/// * Falls back to century expressions (e.g. "18th century"), returning
///   an `OriginYear::EstimatedRange` with (min, max).
/// * If multiple four-digit years are present but the same, still returns
///   ExactYear; if they differ, returns `None` to avoid guessing.
pub fn enrich_origin_year(cmd: &mut CreateProductCommand) {
    let text = extract_text(cmd);
    let lower = text.to_lowercase();

    // Collect all plausible four-digit years.
    let years: Vec<i32> = YEAR_RE
        .find_iter(&text)
        .filter_map(|m| m.as_str().parse::<i32>().ok())
        .filter(|&y| (1000..=2025).contains(&y))
        .collect();

    // If there is exactly one distinct year, accept it.
    if !years.is_empty() {
        let first = years[0];
        if years.iter().all(|&y| y == first) {
            cmd.origin_year = Some(OriginYear::ExactYear(Year::from(first)));
            return;
        }
        // Multiple distinct years → ambiguous, skip.
        return;
    }

    // Fallback: century expressions (longest match first for roman numerals).
    for &(pattern, min, max) in CENTURY_PATTERNS.iter().rev() {
        if lower.contains(pattern) {
            cmd.origin_year = Some(OriginYear::EstimatedRange(common::year::YearRange {
                min: Some(Year::from(min)),
                max: Some(Year::from(max)),
            }));
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// enrich_authenticity
// ---------------------------------------------------------------------------

/// High-confidence keyword sets for each authenticity variant.
/// Each entry is `(keyword, Authenticity)`.  Checked longest-first per group.
const AUTHENTICITY_KEYWORDS: &[(&str, Authenticity)] = &[
    // Reproduction (DE/EN/FR/ES/IT)
    ("reproduktion", Authenticity::Reproduction),
    ("reproduction", Authenticity::Reproduction),
    ("nachbildung", Authenticity::Reproduction),
    ("riproduzione", Authenticity::Reproduction),
    ("reproducción", Authenticity::Reproduction),
    // Later copy
    ("spätere kopie", Authenticity::LaterCopy),
    ("later copy", Authenticity::LaterCopy),
    ("copie tardive", Authenticity::LaterCopy),
    ("copie ultérieure", Authenticity::LaterCopy),
    ("copia posterior", Authenticity::LaterCopy),
    ("copia successiva", Authenticity::LaterCopy),
    // Original (DE/EN/FR/ES/IT) – checked after the others to avoid
    // false-positives when a description says "Originalzustand" etc.
    ("originalzustand", Authenticity::Original),
    ("original condition", Authenticity::Original),
    ("état original", Authenticity::Original),
    ("estado original", Authenticity::Original),
    ("stato originale", Authenticity::Original),
    ("condizione originale", Authenticity::Original),
];

pub fn enrich_authenticity(cmd: &mut CreateProductCommand) {
    let text = extract_text(cmd).to_lowercase();

    for &(keyword, value) in AUTHENTICITY_KEYWORDS {
        if text.contains(keyword) {
            cmd.authenticity = value;
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// enrich_condition
// ---------------------------------------------------------------------------

const CONDITION_KEYWORDS: &[(&str, Condition)] = &[
    // Excellent (DE/EN/FR/ES/IT)
    ("ausgezeichnetem zustand", Condition::Excellent),
    ("ausgezeichneter zustand", Condition::Excellent),
    ("hervorragendem zustand", Condition::Excellent),
    ("hervorragender zustand", Condition::Excellent),
    ("excellent condition", Condition::Excellent),
    ("excellent état", Condition::Excellent),
    ("excelente estado", Condition::Excellent),
    ("eccellente condizione", Condition::Excellent),
    ("condizioni eccellenti", Condition::Excellent),
    ("parfait état", Condition::Excellent),
    // Great / Very Good
    ("sehr gutem zustand", Condition::Great),
    ("sehr guter zustand", Condition::Great),
    ("very good condition", Condition::Great),
    ("très bon état", Condition::Great),
    ("muy buen estado", Condition::Great),
    ("ottime condizioni", Condition::Great),
    ("ottimo stato", Condition::Great),
    // Good
    ("gutem zustand", Condition::Good),
    ("guter zustand", Condition::Good),
    ("good condition", Condition::Good),
    ("bon état", Condition::Good),
    ("buen estado", Condition::Good),
    ("buone condizioni", Condition::Good),
    ("buono stato", Condition::Good),
    // Fair
    ("mäßigem zustand", Condition::Fair),
    ("mäßiger zustand", Condition::Fair),
    ("fair condition", Condition::Fair),
    ("état passable", Condition::Fair),
    ("estado aceptable", Condition::Fair),
    ("condizioni discrete", Condition::Fair),
    // Poor
    ("schlechtem zustand", Condition::Poor),
    ("schlechter zustand", Condition::Poor),
    ("poor condition", Condition::Poor),
    ("mauvais état", Condition::Poor),
    ("mal estado", Condition::Poor),
    ("cattive condizioni", Condition::Poor),
    ("cattivo stato", Condition::Poor),
];

pub fn enrich_condition(cmd: &mut CreateProductCommand) {
    let text = extract_text(cmd).to_lowercase();

    for &(keyword, value) in CONDITION_KEYWORDS {
        if text.contains(keyword) {
            cmd.condition = value;
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// enrich_provenance
// ---------------------------------------------------------------------------

const PROVENANCE_KEYWORDS: &[(&str, Provenance)] = &[
    // Complete
    ("vollständige provenienz", Provenance::Complete),
    ("vollständiger provenienz", Provenance::Complete),
    ("lückenlose provenienz", Provenance::Complete),
    ("lückenloser provenienz", Provenance::Complete),
    ("complete provenance", Provenance::Complete),
    ("full provenance", Provenance::Complete),
    ("provenance complète", Provenance::Complete),
    ("procedencia completa", Provenance::Complete),
    ("provenienza completa", Provenance::Complete),
    // Partial
    ("teilweise provenienz", Provenance::Partial),
    ("teilweiser provenienz", Provenance::Partial),
    ("partial provenance", Provenance::Partial),
    ("provenance partielle", Provenance::Partial),
    ("procedencia parcial", Provenance::Partial),
    ("provenienza parziale", Provenance::Partial),
    // No provenance
    ("ohne provenienz", Provenance::None),
    ("no provenance", Provenance::None),
    ("sans provenance", Provenance::None),
    ("sin procedencia", Provenance::None),
    ("senza provenienza", Provenance::None),
];

pub fn enrich_provenance(cmd: &mut CreateProductCommand) {
    let text = extract_text(cmd).to_lowercase();

    for &(keyword, value) in PROVENANCE_KEYWORDS {
        if text.contains(keyword) {
            cmd.provenance = value;
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// enrich_restoration
// ---------------------------------------------------------------------------

const RESTORATION_KEYWORDS: &[(&str, Restoration)] = &[
    // Major
    ("umfangreiche restaurierung", Restoration::Major),
    ("umfangreicher restaurierung", Restoration::Major),
    ("aufwendig restauriert", Restoration::Major),
    ("major restoration", Restoration::Major),
    ("extensively restored", Restoration::Major),
    ("restauration majeure", Restoration::Major),
    ("entièrement restauré", Restoration::Major),
    ("restauración mayor", Restoration::Major),
    ("ampliamente restaurado", Restoration::Major),
    ("restauro importante", Restoration::Major),
    ("ampiamente restaurato", Restoration::Major),
    // Minor
    ("kleine restaurierung", Restoration::Minor),
    ("kleiner restaurierung", Restoration::Minor),
    ("leicht restauriert", Restoration::Minor),
    ("minor restoration", Restoration::Minor),
    ("slightly restored", Restoration::Minor),
    ("petite restauration", Restoration::Minor),
    ("légèrement restauré", Restoration::Minor),
    ("restauración menor", Restoration::Minor),
    ("ligeramente restaurado", Restoration::Minor),
    ("piccolo restauro", Restoration::Minor),
    ("leggermente restaurato", Restoration::Minor),
    // None
    ("unrestauriert", Restoration::None),
    ("nicht restauriert", Restoration::None),
    ("unrestored", Restoration::None),
    ("non restauré", Restoration::None),
    ("sin restaurar", Restoration::None),
    ("no restaurado", Restoration::None),
    ("non restaurato", Restoration::None),
];

pub fn enrich_restoration(cmd: &mut CreateProductCommand) {
    let text = extract_text(cmd).to_lowercase();

    for &(keyword, value) in RESTORATION_KEYWORDS {
        if text.contains(keyword) {
            cmd.restoration = value;
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// classify_period & classify_category – keyword matching from pre-loaded data
// ---------------------------------------------------------------------------

/// Matches the product **title** (not description) against a pre-built keyword
/// index.  Each entry maps a lowercased keyword to a `PeriodId`.  The index is
/// sorted longest-keyword-first so the most specific match wins.
pub fn classify_period(
    cmd: &mut CreateProductCommand,
    period_keywords: &[(String, common::period_key::PeriodId)],
) {
    let title = cmd.native_title.payload.as_ref().to_lowercase();
    for (keyword, period_id) in period_keywords {
        if title.contains(keyword.as_str()) {
            cmd.period_id = Some(period_id.clone());
            return;
        }
    }
}

/// Matches the product **title** (not description) against a pre-built keyword
/// index.  Each entry maps a lowercased keyword to a `CategoryId`.  The index
/// is sorted longest-keyword-first so the most specific match wins.
pub fn classify_category(
    cmd: &mut CreateProductCommand,
    category_keywords: &[(String, common::category_key::CategoryId)],
) {
    let title = cmd.native_title.payload.as_ref().to_lowercase();
    for (keyword, category_id) in category_keywords {
        if title.contains(keyword.as_str()) {
            cmd.category_id = Some(category_id.clone());
            return;
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::description::Description;
    use crate::core::product_image::ProductImage;
    use crate::core::title::Title;
    use common::language::domain::Language;
    use common::localized::Localized;
    use fake::{Fake, Faker};

    /// Helper: build a `CreateProductCommand` with the given title and
    /// optional description text.
    fn cmd_with(title: &str, description: Option<&str>) -> CreateProductCommand {
        let mut cmd = Faker.fake::<CreateProductCommand>();
        cmd.native_title = Localized::new(Language::De, Title::from(title));
        cmd.native_description =
            description.map(|d| Localized::new(Language::De, Description::from(d)));
        cmd
    }

    // =======================================================================
    // classify_images
    // =======================================================================

    mod classify_images {
        use super::*;

        fn make_images(n: usize) -> Vec<ProductImage> {
            fake::vec![ProductImage; n]
        }

        #[test]
        fn should_flag_nazi_germany_when_title_contains_drittes_reich() {
            let mut cmd = cmd_with("Orden aus dem Dritten Reich", None);
            cmd.images = make_images(3);
            super::classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::NaziGermany);
            }
        }

        #[test]
        fn should_flag_nazi_germany_when_description_contains_hakenkreuz() {
            let mut cmd = cmd_with("Antike Medaille", Some("Medaille mit Hakenkreuz-Symbol"));
            cmd.images = make_images(2);
            super::classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::NaziGermany);
            }
        }

        #[test]
        fn should_flag_nazi_germany_when_english_third_reich() {
            let mut cmd = cmd_with("Medal from the Third Reich", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_french_croix_gammee() {
            let mut cmd = cmd_with("Médaille avec croix gammée", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_spanish_tercer_reich() {
            let mut cmd = cmd_with("Medalla del Tercer Reich", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_italian_terzo_reich() {
            let mut cmd = cmd_with("Medaglia del Terzo Reich", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_nsdap_mentioned() {
            let mut cmd = cmd_with("NSDAP Abzeichen", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_swastika_in_english() {
            let mut cmd = cmd_with("Badge with Swastika", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_set_none_when_no_nazi_keywords() {
            let mut cmd = cmd_with("Barocker Schrank aus dem 18. Jahrhundert", None);
            cmd.images = make_images(2);
            super::classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::None);
            }
        }

        #[test]
        fn should_set_none_when_no_images() {
            let mut cmd = cmd_with("Drittes Reich Medaille", None);
            cmd.images = vec![];
            super::classify_images(&mut cmd);
            assert!(cmd.images.is_empty());
        }

        #[test]
        fn should_be_case_insensitive_for_nazi_keywords() {
            let mut cmd = cmd_with("medal from the THIRD REICH era", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_keyword_in_description_only() {
            let mut cmd = cmd_with(
                "Antike Medaille",
                Some("Aus der Zeit des Nationalsozialismus"),
            );
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_hitlerjugend_mentioned() {
            let mut cmd = cmd_with("Hitlerjugend Messer", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_germania_nazista_in_italian() {
            let mut cmd = cmd_with("Oggetto della Germania Nazista", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany_when_allemagne_nazie_in_french() {
            let mut cmd = cmd_with("Objet de l'Allemagne nazie", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_not_flag_when_keyword_is_substring_of_another_word() {
            let mut cmd = cmd_with("Some nsdapping decoration", None);
            cmd.images = make_images(1);
            super::classify_images(&mut cmd);
            assert_eq!(cmd.images[0].prohibited_content, ProhibitedContent::None);
        }
    }

    // =======================================================================
    // enrich_origin_year
    // =======================================================================

    mod enrich_origin_year {
        use super::*;
        use common::year::YearRange;

        #[test]
        fn should_extract_exact_year_from_title() {
            let mut cmd = cmd_with("Kabinettschrank 1742 süddeutsch", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::ExactYear(Year::from(1742)))
            );
        }

        #[test]
        fn should_extract_exact_year_from_description() {
            let mut cmd = cmd_with("Antiker Schrank", Some("Hergestellt im Jahre 1850"));
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::ExactYear(Year::from(1850)))
            );
        }

        #[test]
        fn should_not_extract_year_when_multiple_different_years() {
            let mut cmd = cmd_with("Schrank 1742 und 1850", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(cmd.origin_year, None);
        }

        #[test]
        fn should_extract_when_same_year_repeated() {
            let mut cmd = cmd_with("Schrank 1742, datiert 1742", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::ExactYear(Year::from(1742)))
            );
        }

        #[test]
        fn should_extract_century_range_when_18th_century() {
            let mut cmd = cmd_with("18th century cabinet", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::EstimatedRange(YearRange {
                    min: Some(Year::from(1700)),
                    max: Some(Year::from(1799)),
                }))
            );
        }

        #[test]
        fn should_extract_century_range_when_german_19_jahrhundert() {
            let mut cmd = cmd_with("Kommode 19. Jahrhundert", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::EstimatedRange(YearRange {
                    min: Some(Year::from(1800)),
                    max: Some(Year::from(1899)),
                }))
            );
        }

        #[test]
        fn should_extract_century_range_when_french_xviiie_siecle() {
            let mut cmd = cmd_with("Commode XVIIIe siècle", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::EstimatedRange(YearRange {
                    min: Some(Year::from(1700)),
                    max: Some(Year::from(1799)),
                }))
            );
        }

        #[test]
        fn should_extract_century_range_when_spanish_siglo_xix() {
            let mut cmd = cmd_with("Cómoda siglo XIX", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::EstimatedRange(YearRange {
                    min: Some(Year::from(1800)),
                    max: Some(Year::from(1899)),
                }))
            );
        }

        #[test]
        fn should_extract_century_range_when_italian_xviii_secolo() {
            let mut cmd = cmd_with("Cassettone XVIII secolo", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::EstimatedRange(YearRange {
                    min: Some(Year::from(1700)),
                    max: Some(Year::from(1799)),
                }))
            );
        }

        #[test]
        fn should_not_extract_when_no_year_present() {
            let mut cmd = cmd_with("Antiker Schrank", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(cmd.origin_year, None);
        }

        #[test]
        fn should_not_extract_when_year_out_of_range() {
            let mut cmd = cmd_with("Artikelnummer 9999", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(cmd.origin_year, None);
        }

        #[test]
        fn should_prefer_exact_year_over_century() {
            let mut cmd = cmd_with("18th century cabinet, dated 1742", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::ExactYear(Year::from(1742)))
            );
        }

        #[test]
        fn should_extract_year_at_boundary_1000() {
            let mut cmd = cmd_with("Item from 1000 AD", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::ExactYear(Year::from(1000)))
            );
        }

        #[test]
        fn should_extract_year_at_boundary_2025() {
            let mut cmd = cmd_with("Item from 2025", None);
            super::enrich_origin_year(&mut cmd);
            assert_eq!(
                cmd.origin_year,
                Some(OriginYear::ExactYear(Year::from(2025)))
            );
        }
    }

    // =======================================================================
    // enrich_authenticity
    // =======================================================================

    mod enrich_authenticity {
        use super::*;

        #[test]
        fn should_detect_original_from_german_originalzustand() {
            let mut cmd = cmd_with("Schrank im Originalzustand", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Original);
        }

        #[test]
        fn should_detect_original_from_english_original_condition() {
            let mut cmd = cmd_with("Cabinet in original condition", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Original);
        }

        #[test]
        fn should_detect_original_from_french() {
            let mut cmd = cmd_with("Commode en état original", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Original);
        }

        #[test]
        fn should_detect_original_from_spanish() {
            let mut cmd = cmd_with("Cómoda en estado original", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Original);
        }

        #[test]
        fn should_detect_original_from_italian() {
            let mut cmd = cmd_with("Cassettone in stato originale", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Original);
        }

        #[test]
        fn should_detect_reproduction_from_german() {
            let mut cmd = cmd_with("Reproduktion einer Barockkommode", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Reproduction);
        }

        #[test]
        fn should_detect_reproduction_from_english() {
            let mut cmd = cmd_with("Reproduction of a baroque cabinet", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Reproduction);
        }

        #[test]
        fn should_detect_reproduction_from_spanish() {
            let mut cmd = cmd_with("Reproducción de un mueble barroco", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Reproduction);
        }

        #[test]
        fn should_detect_reproduction_from_italian() {
            let mut cmd = cmd_with("Riproduzione di un mobile barocco", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Reproduction);
        }

        #[test]
        fn should_detect_later_copy_from_german() {
            let mut cmd = cmd_with("Spätere Kopie eines Gemäldes", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::LaterCopy);
        }

        #[test]
        fn should_detect_later_copy_from_english() {
            let mut cmd = cmd_with("A later copy of the painting", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::LaterCopy);
        }

        #[test]
        fn should_detect_later_copy_from_french() {
            let mut cmd = cmd_with("Copie ultérieure du tableau", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::LaterCopy);
        }

        #[test]
        fn should_not_detect_when_no_keywords() {
            let mut cmd = cmd_with("Barocker Schrank", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Unknown);
        }

        #[test]
        fn should_be_case_insensitive() {
            let mut cmd = cmd_with("REPRODUKTION eines Gemäldes", None);
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Reproduction);
        }

        #[test]
        fn should_detect_from_description() {
            let mut cmd = cmd_with("Antiker Schrank", Some("In perfektem Originalzustand"));
            super::enrich_authenticity(&mut cmd);
            assert_eq!(cmd.authenticity, Authenticity::Original);
        }
    }

    // =======================================================================
    // enrich_condition
    // =======================================================================

    mod enrich_condition {
        use super::*;

        #[test]
        fn should_detect_excellent_from_german() {
            let mut cmd = cmd_with("Schrank in ausgezeichnetem Zustand", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Excellent);
        }

        #[test]
        fn should_detect_excellent_from_english() {
            let mut cmd = cmd_with("Cabinet in excellent condition", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Excellent);
        }

        #[test]
        fn should_detect_excellent_from_french() {
            let mut cmd = cmd_with("Commode en parfait état", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Excellent);
        }

        #[test]
        fn should_detect_excellent_from_spanish() {
            let mut cmd = cmd_with("Cómoda en excelente estado", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Excellent);
        }

        #[test]
        fn should_detect_excellent_from_italian() {
            let mut cmd = cmd_with("Cassettone in condizioni eccellenti", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Excellent);
        }

        #[test]
        fn should_detect_great_from_german() {
            let mut cmd = cmd_with("Kommode in sehr gutem Zustand", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Great);
        }

        #[test]
        fn should_detect_great_from_english() {
            let mut cmd = cmd_with("Table in very good condition", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Great);
        }

        #[test]
        fn should_detect_great_from_french() {
            let mut cmd = cmd_with("Table en très bon état", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Great);
        }

        #[test]
        fn should_detect_great_from_spanish() {
            let mut cmd = cmd_with("Mesa en muy buen estado", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Great);
        }

        #[test]
        fn should_detect_great_from_italian() {
            let mut cmd = cmd_with("Tavolo in ottime condizioni", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Great);
        }

        #[test]
        fn should_detect_good_from_german() {
            let mut cmd = cmd_with("Stuhl in gutem Zustand", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Good);
        }

        #[test]
        fn should_detect_good_from_english() {
            let mut cmd = cmd_with("Chair in good condition", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Good);
        }

        #[test]
        fn should_detect_fair_from_english() {
            let mut cmd = cmd_with("Chair in fair condition", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Fair);
        }

        #[test]
        fn should_detect_poor_from_german() {
            let mut cmd = cmd_with("Stuhl in schlechtem Zustand", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Poor);
        }

        #[test]
        fn should_detect_poor_from_english() {
            let mut cmd = cmd_with("Chair in poor condition", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Poor);
        }

        #[test]
        fn should_detect_poor_from_french() {
            let mut cmd = cmd_with("Chaise en mauvais état", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Poor);
        }

        #[test]
        fn should_not_detect_when_no_keywords() {
            let mut cmd = cmd_with("Antiker Schrank", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Unknown);
        }

        #[test]
        fn should_prefer_more_specific_match() {
            // "sehr guter Zustand" should match Great, not Good
            let mut cmd = cmd_with("Schrank in sehr gutem Zustand", None);
            super::enrich_condition(&mut cmd);
            assert_eq!(cmd.condition, Condition::Great);
        }
    }

    // =======================================================================
    // enrich_provenance
    // =======================================================================

    mod enrich_provenance {
        use super::*;

        #[test]
        fn should_detect_complete_from_german() {
            let mut cmd = cmd_with("Gemälde mit vollständiger Provenienz", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Complete);
        }

        #[test]
        fn should_detect_complete_from_english() {
            let mut cmd = cmd_with("Painting with complete provenance", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Complete);
        }

        #[test]
        fn should_detect_complete_from_french() {
            let mut cmd = cmd_with("Tableau avec provenance complète", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Complete);
        }

        #[test]
        fn should_detect_complete_from_spanish() {
            let mut cmd = cmd_with("Cuadro con procedencia completa", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Complete);
        }

        #[test]
        fn should_detect_complete_from_italian() {
            let mut cmd = cmd_with("Quadro con provenienza completa", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Complete);
        }

        #[test]
        fn should_detect_partial_from_german() {
            let mut cmd = cmd_with("Vase mit teilweiser Provenienz", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Partial);
        }

        #[test]
        fn should_detect_partial_from_english() {
            let mut cmd = cmd_with("Vase with partial provenance", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Partial);
        }

        #[test]
        fn should_detect_no_provenance_from_german() {
            let mut cmd = cmd_with("Skulptur ohne Provenienz", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::None);
        }

        #[test]
        fn should_detect_no_provenance_from_english() {
            let mut cmd = cmd_with("Sculpture with no provenance", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::None);
        }

        #[test]
        fn should_detect_no_provenance_from_french() {
            let mut cmd = cmd_with("Sculpture sans provenance", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::None);
        }

        #[test]
        fn should_not_detect_when_no_keywords() {
            let mut cmd = cmd_with("Antike Skulptur", None);
            super::enrich_provenance(&mut cmd);
            assert_eq!(cmd.provenance, Provenance::Unknown);
        }
    }

    // =======================================================================
    // enrich_restoration
    // =======================================================================

    mod enrich_restoration {
        use super::*;

        #[test]
        fn should_detect_major_from_german() {
            let mut cmd = cmd_with("Schrank mit umfangreicher Restaurierung", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Major);
        }

        #[test]
        fn should_detect_major_from_english() {
            let mut cmd = cmd_with("Cabinet with major restoration", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Major);
        }

        #[test]
        fn should_detect_major_from_french() {
            let mut cmd = cmd_with("Commode avec restauration majeure", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Major);
        }

        #[test]
        fn should_detect_major_from_spanish() {
            let mut cmd = cmd_with("Cómoda con restauración mayor", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Major);
        }

        #[test]
        fn should_detect_major_from_italian() {
            let mut cmd = cmd_with("Cassettone con restauro importante", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Major);
        }

        #[test]
        fn should_detect_minor_from_german() {
            let mut cmd = cmd_with("Tisch leicht restauriert", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Minor);
        }

        #[test]
        fn should_detect_minor_from_english() {
            let mut cmd = cmd_with("Table with minor restoration", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Minor);
        }

        #[test]
        fn should_detect_minor_from_french() {
            let mut cmd = cmd_with("Table légèrement restauré", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Minor);
        }

        #[test]
        fn should_detect_minor_from_spanish() {
            let mut cmd = cmd_with("Mesa ligeramente restaurado", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Minor);
        }

        #[test]
        fn should_detect_minor_from_italian() {
            let mut cmd = cmd_with("Tavolo leggermente restaurato", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Minor);
        }

        #[test]
        fn should_detect_none_from_german() {
            let mut cmd = cmd_with("Schrank unrestauriert", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::None);
        }

        #[test]
        fn should_detect_none_from_english() {
            let mut cmd = cmd_with("Cabinet unrestored", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::None);
        }

        #[test]
        fn should_detect_none_from_french() {
            let mut cmd = cmd_with("Commode non restauré", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::None);
        }

        #[test]
        fn should_detect_none_from_spanish() {
            let mut cmd = cmd_with("Cómoda sin restaurar", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::None);
        }

        #[test]
        fn should_detect_none_from_italian() {
            let mut cmd = cmd_with("Cassettone non restaurato", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::None);
        }

        #[test]
        fn should_not_detect_when_no_keywords() {
            let mut cmd = cmd_with("Antiker Schrank", None);
            super::enrich_restoration(&mut cmd);
            assert_eq!(cmd.restoration, Restoration::Unknown);
        }
    }

    // =======================================================================
    // classify_period & classify_category
    // =======================================================================

    mod classify_period {
        use super::*;
        use common::period_key::PeriodId;

        fn period_index() -> Vec<(String, PeriodId)> {
            vec![
                ("baroque".into(), PeriodId::raw("period-baroque")),
                ("barock".into(), PeriodId::raw("period-baroque")),
                (
                    "baroque tardif".into(),
                    PeriodId::raw("period-late-baroque"),
                ),
                ("spätbarock".into(), PeriodId::raw("period-late-baroque")),
                ("renaissance".into(), PeriodId::raw("period-renaissance")),
                ("art deco".into(), PeriodId::raw("period-art-deco")),
                ("art déco".into(), PeriodId::raw("period-art-deco")),
                ("jugendstil".into(), PeriodId::raw("period-art-nouveau")),
                ("art nouveau".into(), PeriodId::raw("period-art-nouveau")),
            ]
        }

        #[test]
        fn should_classify_period_when_title_contains_keyword() {
            let mut cmd = cmd_with("Barocker Schrank aus Süddeutschland", None);
            super::classify_period(&mut cmd, &period_index());
            assert_eq!(cmd.period_id, Some(PeriodId::raw("period-baroque")));
        }

        #[test]
        fn should_classify_period_for_english_keyword() {
            let mut cmd = cmd_with("Baroque cabinet from France", None);
            super::classify_period(&mut cmd, &period_index());
            assert_eq!(cmd.period_id, Some(PeriodId::raw("period-baroque")));
        }

        #[test]
        fn should_prefer_longer_keyword_match_for_specificity() {
            let mut cmd = cmd_with("Kommode Spätbarock", None);
            let mut index = period_index();
            // Sort longest-first so "spätbarock" matches before "barock"
            index.sort_by_key(|b| std::cmp::Reverse(b.0.len()));
            super::classify_period(&mut cmd, &index);
            assert_eq!(cmd.period_id, Some(PeriodId::raw("period-late-baroque")));
        }

        #[test]
        fn should_not_set_period_when_no_keyword_matches() {
            let mut cmd = cmd_with("Antiker Tisch aus dem 18. Jahrhundert", None);
            super::classify_period(&mut cmd, &period_index());
            assert_eq!(cmd.period_id, None);
        }

        #[test]
        fn should_not_set_period_when_index_is_empty() {
            let mut cmd = cmd_with("Barocker Schrank", None);
            super::classify_period(&mut cmd, &[]);
            assert_eq!(cmd.period_id, None);
        }

        #[test]
        fn should_match_case_insensitively() {
            let mut cmd = cmd_with("ART DECO Lampe", None);
            super::classify_period(&mut cmd, &period_index());
            assert_eq!(cmd.period_id, Some(PeriodId::raw("period-art-deco")));
        }

        #[test]
        fn should_only_match_title_not_description() {
            let mut cmd = cmd_with("Antiker Tisch", Some("Im Stil des Barock"));
            super::classify_period(&mut cmd, &period_index());
            assert_eq!(cmd.period_id, None);
        }
    }

    mod classify_category {
        use super::*;
        use common::category_key::CategoryId;

        fn category_index() -> Vec<(String, CategoryId)> {
            vec![
                ("gemälde".into(), CategoryId::raw("cat-paintings")),
                ("painting".into(), CategoryId::raw("cat-paintings")),
                ("peinture".into(), CategoryId::raw("cat-paintings")),
                ("schrank".into(), CategoryId::raw("cat-furniture")),
                ("cabinet".into(), CategoryId::raw("cat-furniture")),
                ("armoire".into(), CategoryId::raw("cat-furniture")),
                ("skulptur".into(), CategoryId::raw("cat-sculpture")),
                ("sculpture".into(), CategoryId::raw("cat-sculpture")),
            ]
        }

        #[test]
        fn should_classify_category_when_title_contains_keyword() {
            let mut cmd = cmd_with("Barocker Schrank aus dem 18. Jahrhundert", None);
            super::classify_category(&mut cmd, &category_index());
            assert_eq!(cmd.category_id, Some(CategoryId::raw("cat-furniture")));
        }

        #[test]
        fn should_classify_category_for_english_keyword() {
            let mut cmd = cmd_with("Antique oil painting from 1850", None);
            super::classify_category(&mut cmd, &category_index());
            assert_eq!(cmd.category_id, Some(CategoryId::raw("cat-paintings")));
        }

        #[test]
        fn should_classify_category_for_french_keyword() {
            let mut cmd = cmd_with("Ancienne armoire en chêne", None);
            super::classify_category(&mut cmd, &category_index());
            assert_eq!(cmd.category_id, Some(CategoryId::raw("cat-furniture")));
        }

        #[test]
        fn should_not_set_category_when_no_keyword_matches() {
            let mut cmd = cmd_with("Antiker Gegenstand", None);
            super::classify_category(&mut cmd, &category_index());
            assert_eq!(cmd.category_id, None);
        }

        #[test]
        fn should_not_set_category_when_index_is_empty() {
            let mut cmd = cmd_with("Gemälde aus dem 18. Jahrhundert", None);
            super::classify_category(&mut cmd, &[]);
            assert_eq!(cmd.category_id, None);
        }

        #[test]
        fn should_match_case_insensitively() {
            let mut cmd = cmd_with("Antike SKULPTUR aus Bronze", None);
            super::classify_category(&mut cmd, &category_index());
            assert_eq!(cmd.category_id, Some(CategoryId::raw("cat-sculpture")));
        }

        #[test]
        fn should_only_match_title_not_description() {
            let mut cmd = cmd_with("Antiker Gegenstand", Some("Dies ist ein Gemälde"));
            super::classify_category(&mut cmd, &category_index());
            assert_eq!(cmd.category_id, None);
        }
    }
}
