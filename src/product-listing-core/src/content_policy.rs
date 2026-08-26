use strum::IntoEnumIterator;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, strum_macros::EnumIter)]
pub enum SensitiveContentCategory {
    NaziGermany,
}

impl SensitiveContentCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NaziGermany => "NAZI_GERMANY",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|category| category.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ContentPolicyDecision {
    Allowed,
    RequiresConsent(SensitiveContentCategory),
}

impl ContentPolicyDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "ALLOWED",
            Self::RequiresConsent(_) => "REQUIRES_CONSENT",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "ALLOWED" => Some(Self::Allowed),
            "REQUIRES_CONSENT" => None,
            _ => None,
        }
    }
}

pub fn may_show_product_listing_images(
    decision: Option<ContentPolicyDecision>,
    show_unassessed_or_sensitive_content: bool,
) -> bool {
    show_unassessed_or_sensitive_content || matches!(decision, Some(ContentPolicyDecision::Allowed))
}

// ---------------------------------------------------------------------------
// Listing text assessment
// ---------------------------------------------------------------------------

/// High-confidence terms that strongly indicate Nazi-era imagery or
/// paraphernalia.  Every keyword is lowercase for case-insensitive matching.
///
/// The list is organized by language and category.  Multi-word phrases act as
/// their own word boundaries because spaces are not alphanumeric; single tokens
/// are guarded by `contains_keyword` to avoid substring collisions.
const NAZI_KEYWORDS: &[&str] = &[
    // ── GERMAN ── core ideology & regime names ────────────────────────────
    "drittes reich",
    "dritten reich",
    "dritte reich",
    "3. reich",
    "iii. reich",
    "iii reich",
    "nationalsozialismus",
    "nationalsozialistisch",
    "nationalsozialistische",
    "nationalsozialisten",
    "nazideutschland",
    // ── GERMAN ── symbols & iconography ──────────────────────────────────
    "hakenkreuz",
    "hakenkreuzfahne",
    "hakenkreuzflagge",
    "hakenkreuzabzeichen",
    "reichsadler",
    "schwarze sonne",
    "ss-rune",
    "lebensrune",
    "todesrune",
    "sig-rune",
    // ── GERMAN ── organizations ──────────────────────────────────────────
    "nsdap",
    "schutzstaffel",
    "sturmabteilung",
    "hitlerjugend",
    "bund deutscher mädel",
    "deutsches jungvolk",
    "reichsarbeitsdienst",
    "waffen-ss",
    "waffen ss",
    "leibstandarte",
    "totenkopfverbände",
    "totenkopf-ring",
    "totenkopfring",
    "gestapo",
    "einsatzgruppe",
    "reichssicherheitshauptamt",
    "lebensborn",
    "organisation todt",
    // ── GERMAN ── senior Nazi leadership (unambiguous) ────────────────────
    "adolf hitler",
    "heil hitler",
    "führerhauptquartier",
    "wolfsschanze",
    "reichskanzlei",
    "reichsführer-ss",
    "heinrich himmler",
    "joseph goebbels",
    "hermann göring",
    "hermann goering",
    "rudolf hess",
    "reinhard heydrich",
    "martin bormann",
    "julius streicher",
    "ernst röhm",
    "ernst rohm",
    // ── GERMAN ── events & sites ─────────────────────────────────────────
    "reichsparteitag",
    "reichskristallnacht",
    "kristallnacht",
    // ── ENGLISH ── regime names & ideology ───────────────────────────────
    "third reich",
    "nazi germany",
    "national socialism",
    "nazi party",
    // ── ENGLISH ── symbols ────────────────────────────────────────────────
    "swastika",
    "black sun occult",
    // ── ENGLISH ── organizations ─────────────────────────────────────────
    "hitler youth",
    "waffen ss",
    "waffen-ss",
    "gestapo",
    "ss division",
    "ss insignia",
    "ss uniform",
    "ss badge",
    "ss dagger",
    "ss helmet",
    "ss ring",
    // ── ENGLISH ── leadership ─────────────────────────────────────────────
    "heinrich himmler",
    "joseph goebbels",
    "hermann goering",
    "rudolf hess",
    "reinhard heydrich",
    // ── ENGLISH ── memorabilia descriptors ───────────────────────────────
    "nazi memorabilia",
    "nazi insignia",
    "nazi medal",
    "nazi uniform",
    "nazi dagger",
    "nazi helmet",
    "nazi armband",
    "nazi flag",
    "nazi germany",
    "reich chancellery",
    // ── FRENCH ── ─────────────────────────────────────────────────────────
    "troisième reich",
    "iii° reich",
    "croix gammée",
    "croix gammee",
    "national-socialisme",
    "nationalsocialisme",
    "allemagne nazie",
    "parti nazi",
    "waffen ss",
    "waffen-ss",
    "gestapo",
    "jeunesses hitlériennes",
    "heinrich himmler",
    "joseph goebbels",
    // ── SPANISH ── ────────────────────────────────────────────────────────
    "tercer reich",
    "esvástica",
    "esvastica",
    "svástica",
    "svastica",
    "cruz gamada",
    "nacionalsocialismo",
    "alemania nazi",
    "partido nazi",
    "waffen ss",
    "waffen-ss",
    "gestapo",
    "juventudes hitlerianas",
    // ── ITALIAN ── ────────────────────────────────────────────────────────
    "terzo reich",
    "svastica",
    "nazionalsocialismo",
    "germania nazista",
    "partito nazista",
    "waffen ss",
    "waffen-ss",
    "gestapo",
    "gioventù hitleriana",
    // ── DUTCH ── ──────────────────────────────────────────────────────────
    "derde rijk",
    "hakenkruis",
    "nationaal-socialisme",
    "naziduitsland",
    "nazi-duitsland",
    "hitleriaanse jeugd",
    "waffen ss",
    "gestapo",
    // ── PORTUGUESE ── ────────────────────────────────────────────────────
    "terceiro reich",
    "suástica",
    "suastica",
    "nacional-socialismo",
    "alemanha nazista",
    "partido nazista",
    "waffen ss",
    "gestapo",
    // ── POLISH ── ────────────────────────────────────────────────────────
    "trzecia rzesza",
    "swastyka",
    "narodowy socjalizm",
    "gestapo",
    "waffen ss",
    // ── CZECH / SLOVAK ── ────────────────────────────────────────────────
    "třetí říše",
    "nacistické německo",
    "svastika",
    "gestapo",
    "waffen ss",
];

/// Returns `true` when `text` (expected to already be lowercase) contains
/// `keyword` surrounded by non-alphanumeric boundaries (or string edges).
///
/// This prevents partial-word false positives, e.g. "nsdap" matching inside
/// "nsdapper".  Multi-word keywords naturally include spaces which are not
/// alphanumeric and therefore act as their own boundaries.
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

/// Assesses source listing text. Blank or absent text is not an assessment.
pub fn assess_listing_text(
    title: Option<&str>,
    description: Option<&str>,
) -> Option<ContentPolicyDecision> {
    let parts = [title, description]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();

    if parts.is_empty() {
        return None;
    }

    let text = parts.join(" ").to_lowercase();
    if NAZI_KEYWORDS
        .iter()
        .any(|keyword| contains_keyword(&text, keyword))
    {
        Some(ContentPolicyDecision::RequiresConsent(
            SensitiveContentCategory::NaziGermany,
        ))
    } else {
        Some(ContentPolicyDecision::Allowed)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_keep_canonical_content_policy_identifiers_exact() {
        assert_eq!(ContentPolicyDecision::Allowed.as_str(), "ALLOWED");
        assert_eq!(
            ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany).as_str(),
            "REQUIRES_CONSENT"
        );
        assert_eq!(
            SensitiveContentCategory::NaziGermany.as_str(),
            "NAZI_GERMANY"
        );
        assert_eq!(
            ContentPolicyDecision::from_code("ALLOWED"),
            Some(ContentPolicyDecision::Allowed)
        );
        assert_eq!(ContentPolicyDecision::from_code("allowed"), None);
        assert_eq!(ContentPolicyDecision::from_code("UNKNOWN"), None);
        assert_eq!(ContentPolicyDecision::from_code("NONE"), None);
        assert_eq!(
            SensitiveContentCategory::from_code("NAZI_GERMANY"),
            Some(SensitiveContentCategory::NaziGermany)
        );
        assert_eq!(SensitiveContentCategory::from_code("nazi_germany"), None);
        assert_eq!(
            SensitiveContentCategory::iter()
                .map(SensitiveContentCategory::as_str)
                .collect::<HashSet<_>>()
                .len(),
            SensitiveContentCategory::iter().count(),
        );
    }

    #[rstest::rstest]
    #[case(None, false, false)]
    #[case(None, true, true)]
    #[case(Some(ContentPolicyDecision::Allowed), false, true)]
    #[case(Some(ContentPolicyDecision::Allowed), true, true)]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        false,
        false
    )]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        true,
        true
    )]
    fn should_apply_content_visibility_policy(
        #[case] decision: Option<ContentPolicyDecision>,
        #[case] preference: bool,
        #[case] visible: bool,
    ) {
        assert_eq!(
            may_show_product_listing_images(decision, preference),
            visible
        );
    }

    // =======================================================================
    // Safe / benign listings – must NOT be flagged
    // =======================================================================

    mod should_not_flag {
        use super::*;

        #[test]
        fn should_return_none_when_no_nazi_keywords() {
            assert_eq!(
                assess_listing_text(Some("Barocker Schrank aus dem 18. Jahrhundert"), None),
                Some(ContentPolicyDecision::Allowed)
            );
        }

        #[test]
        fn should_return_no_assessment_when_title_is_empty() {
            assert_eq!(assess_listing_text(Some(""), None), None);
        }

        #[test]
        fn should_not_flag_when_keyword_is_substring_of_another_word() {
            // "nsdapping" contains "nsdap" as prefix but must not match
            assert_eq!(
                assess_listing_text(Some("Some nsdapping decoration"), None),
                Some(ContentPolicyDecision::Allowed)
            );
        }

        #[test]
        fn should_not_flag_antique_furniture_listing() {
            assert_eq!(
                assess_listing_text(
                    Some("Biedermeier Kommode 19. Jahrhundert Mahagoni"),
                    Some("Sehr gut erhalten, aus südddeutschem Privatbesitz")
                ),
                Some(ContentPolicyDecision::Allowed)
            );
        }

        #[test]
        fn should_not_flag_iron_cross_ww1_listing() {
            // An Iron Cross can be WWI – no additional Nazi qualifier here
            assert_eq!(
                assess_listing_text(Some("Eisernes Kreuz 1914 Erster Weltkrieg"), None),
                Some(ContentPolicyDecision::Allowed)
            );
        }

        #[test]
        fn should_not_flag_generic_eagle_sculpture() {
            assert_eq!(
                assess_listing_text(Some("Bronze Adlerfigur Jugendstil"), None),
                Some(ContentPolicyDecision::Allowed)
            );
        }

        #[test]
        fn should_not_flag_general_ww2_book() {
            assert_eq!(
                assess_listing_text(
                    Some("Der Zweite Weltkrieg – Geschichte und Ursachen"),
                    Some("Umfassendes Werk über den Verlauf des Zweiten Weltkriegs")
                ),
                Some(ContentPolicyDecision::Allowed)
            );
        }

        #[test]
        fn should_not_flag_word_ending_with_keyword_fragment() {
            // plain "kreuz" alone must NOT flag
            assert_eq!(
                assess_listing_text(Some("Antikes Silberkreuz"), None),
                Some(ContentPolicyDecision::Allowed)
            );
        }

        #[test]
        fn should_not_flag_generic_military_uniform() {
            assert_eq!(
                assess_listing_text(Some("Uniform Bundeswehr 1970er Jahre"), None),
                Some(ContentPolicyDecision::Allowed)
            );
        }
    }

    // =======================================================================
    // German-language Nazi listings – MUST be flagged
    // =======================================================================

    mod german {
        use super::*;

        #[test]
        fn should_flag_when_title_contains_drittes_reich() {
            assert_eq!(
                assess_listing_text(Some("Orden aus dem Dritten Reich"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_when_title_contains_dritte_reich_variant() {
            assert_eq!(
                assess_listing_text(Some("Abzeichen Dritte Reich 1940"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_when_title_contains_3_punkt_reich() {
            assert_eq!(
                assess_listing_text(Some("Medaille 3. Reich Silber Original"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_when_title_contains_iii_punkt_reich() {
            assert_eq!(
                assess_listing_text(Some("Feldmütze III. Reich WWII"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_when_description_contains_hakenkreuz() {
            assert_eq!(
                assess_listing_text(
                    Some("Antike Medaille"),
                    Some("Medaille mit Hakenkreuz-Symbol graviert")
                ),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_hakenkreuzfahne() {
            assert_eq!(
                assess_listing_text(Some("Hakenkreuzfahne original 1935"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_reichsadler() {
            assert_eq!(
                assess_listing_text(Some("Reichsadler Briefbeschwerer Bronze 1938"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_nsdap_badge() {
            assert_eq!(
                assess_listing_text(Some("NSDAP Abzeichen 1935 Original"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_hitlerjugend_knife() {
            assert_eq!(
                assess_listing_text(Some("Hitlerjugend Messer mit Scheide 1935"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_schutzstaffel() {
            assert_eq!(
                assess_listing_text(Some("Schutzstaffel Dienstglas 8x30"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_sturmabteilung() {
            assert_eq!(
                assess_listing_text(Some("Sturmabteilung SA Dolch M1933"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_waffen_ss_with_hyphen() {
            assert_eq!(
                assess_listing_text(Some("Waffen-SS Feldmütze WWII Original"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_waffen_ss_without_hyphen() {
            assert_eq!(
                assess_listing_text(Some("Waffen SS Uniformjacke 1943"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_totenkopfring() {
            assert_eq!(
                assess_listing_text(Some("SS Totenkopfring Silber Replik"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_gestapo_badge() {
            assert_eq!(
                assess_listing_text(Some("Gestapo Dienstmarke original"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_nationalsozialismus_in_description() {
            assert_eq!(
                assess_listing_text(
                    Some("Antike Medaille Konvolut"),
                    Some("Aus der Zeit des Nationalsozialismus, guter Zustand")
                ),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_bund_deutscher_maedel() {
            assert_eq!(
                assess_listing_text(Some("Bund Deutscher Mädel Abzeichen BDM"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_ss_rune() {
            assert_eq!(
                assess_listing_text(Some("SS-Rune Silber Anhänger Original"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_schwarze_sonne() {
            assert_eq!(
                assess_listing_text(Some("Schwarze Sonne Wandteller Keramik"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_adolf_hitler_portrait() {
            assert_eq!(
                assess_listing_text(Some("Ölgemälde Porträt Adolf Hitler signiert"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_heil_hitler_inscription() {
            assert_eq!(
                assess_listing_text(Some("Postkarte mit Aufschrift Heil Hitler 1937"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_heinrich_himmler_signature() {
            assert_eq!(
                assess_listing_text(Some("Autogramm Heinrich Himmler"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_reichsparteitag_photo() {
            assert_eq!(
                assess_listing_text(Some("Originalfoto Reichsparteitag Nürnberg 1937"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_leibstandarte() {
            assert_eq!(
                assess_listing_text(
                    Some("Leibstandarte SS Adolf Hitler Manschettenknöpfe"),
                    None
                ),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_kristallnacht_photo() {
            assert_eq!(
                assess_listing_text(
                    Some("Originaldokument Reichskristallnacht November 1938"),
                    None
                ),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // English-language Nazi listings – MUST be flagged
    // =======================================================================

    mod english {
        use super::*;

        #[test]
        fn should_flag_third_reich() {
            assert_eq!(
                assess_listing_text(Some("Medal from the Third Reich 1939"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_nazi_germany() {
            assert_eq!(
                assess_listing_text(Some("Badge from Nazi Germany WWII"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_swastika() {
            assert_eq!(
                assess_listing_text(Some("Bronze pendant with Swastika symbol"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_nazi_medal_lot() {
            assert_eq!(
                assess_listing_text(Some("Nazi medal lot – original WWII collection"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_hitler_youth_dagger() {
            assert_eq!(
                assess_listing_text(Some("Hitler Youth Dagger HJ 1937 original"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_waffen_ss_helmet() {
            assert_eq!(
                assess_listing_text(Some("Waffen-SS Steel Helmet M42 WWII"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_ss_dagger() {
            assert_eq!(
                assess_listing_text(Some("SS Dagger Model 1936 with scabbard"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_nazi_armband() {
            assert_eq!(
                assess_listing_text(Some("Original Nazi armband with eagle insignia"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_national_socialism_in_description() {
            assert_eq!(
                assess_listing_text(
                    Some("Historical artefact"),
                    Some("Object related to National Socialism from 1938")
                ),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_gestapo_badge_english() {
            assert_eq!(
                assess_listing_text(Some("Gestapo Secret Police badge original WW2"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_case_insensitive_upper() {
            assert_eq!(
                assess_listing_text(Some("Medal from the THIRD REICH era"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_case_insensitive_mixed() {
            assert_eq!(
                assess_listing_text(Some("Rare Nazi Memorabilia Collection"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_reinhard_heydrich_portrait() {
            assert_eq!(
                assess_listing_text(Some("Portrait photograph Reinhard Heydrich"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // French-language Nazi listings – MUST be flagged
    // =======================================================================

    mod french {
        use super::*;

        #[test]
        fn should_flag_troisieme_reich() {
            assert_eq!(
                assess_listing_text(Some("Médaille du Troisième Reich 1939"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_croix_gammee() {
            assert_eq!(
                assess_listing_text(Some("Médaille avec croix gammée originale"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_allemagne_nazie() {
            assert_eq!(
                assess_listing_text(Some("Objet de l'Allemagne nazie seconde guerre"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_national_socialisme() {
            assert_eq!(
                assess_listing_text(
                    Some("Document historique"),
                    Some("Lié au national-socialisme allemand")
                ),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // Spanish-language Nazi listings – MUST be flagged
    // =======================================================================

    mod spanish {
        use super::*;

        #[test]
        fn should_flag_tercer_reich() {
            assert_eq!(
                assess_listing_text(Some("Medalla del Tercer Reich Segunda Guerra"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_cruz_gamada() {
            assert_eq!(
                assess_listing_text(Some("Colgante de plata con cruz gamada"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_alemania_nazi() {
            assert_eq!(
                assess_listing_text(Some("Insignia de la Alemania Nazi WWII"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // Italian-language Nazi listings – MUST be flagged
    // =======================================================================

    mod italian {
        use super::*;

        #[test]
        fn should_flag_terzo_reich() {
            assert_eq!(
                assess_listing_text(Some("Medaglia del Terzo Reich 1940"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_svastica() {
            assert_eq!(
                assess_listing_text(Some("Pendente argento con svastica originale"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_germania_nazista() {
            assert_eq!(
                assess_listing_text(Some("Oggetto della Germania Nazista seconda guerra"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // Dutch-language Nazi listings – MUST be flagged
    // =======================================================================

    mod dutch {
        use super::*;

        #[test]
        fn should_flag_derde_rijk() {
            assert_eq!(
                assess_listing_text(Some("Medaille Derde Rijk WOII origineel"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_hakenkruis() {
            assert_eq!(
                assess_listing_text(Some("Zilveren hanger met hakenkruis 1938"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // Portuguese-language Nazi listings – MUST be flagged
    // =======================================================================

    mod portuguese {
        use super::*;

        #[test]
        fn should_flag_terceiro_reich() {
            assert_eq!(
                assess_listing_text(Some("Medalha do Terceiro Reich 1939"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_suastica() {
            assert_eq!(
                assess_listing_text(Some("Pendente prata com suástica original"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // Polish-language Nazi listings – MUST be flagged
    // =======================================================================

    mod polish {
        use super::*;

        #[test]
        fn should_flag_trzecia_rzesza() {
            assert_eq!(
                assess_listing_text(Some("Medal Trzecia Rzesza II Wojna Światowa"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_swastyka() {
            assert_eq!(
                assess_listing_text(Some("Rzadka swastyka z 1938 oryginał"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }
    }

    // =======================================================================
    // Edge cases
    // =======================================================================

    mod edge_cases {
        use super::*;

        #[test]
        fn should_flag_when_keyword_only_in_description() {
            assert_eq!(
                assess_listing_text(Some("Antike Uniform"), Some("Schutzstaffel Dienstuniform")),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_when_keyword_at_start_of_text() {
            assert_eq!(
                assess_listing_text(Some("Hakenkreuz Wanddeko"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_when_keyword_at_end_of_text() {
            assert_eq!(
                assess_listing_text(Some("Silbernes Abzeichen Drittes Reich"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_flag_when_multiple_keywords_present() {
            assert_eq!(
                assess_listing_text(Some("NSDAP Abzeichen Waffen-SS Orden Hakenkreuz"), None),
                Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany
                ))
            );
        }

        #[test]
        fn should_return_none_when_description_is_none_and_title_is_benign() {
            assert_eq!(
                assess_listing_text(Some("Völlig harmloses Gemälde"), None),
                Some(ContentPolicyDecision::Allowed)
            );
        }
    }
}
