use crate::prohibited_content::ProhibitedContent;

// ---------------------------------------------------------------------------
// classify_by_text – ProhibitedContent::NaziGermany detection
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

/// Analyses the product title and optional description text and returns the
/// appropriate [`ProhibitedContent`] decision for images.
///
/// * If the text **clearly** suggests Nazi-related content →
///   [`ProhibitedContent::NaziGermany`].
/// * If no indicator is found → [`ProhibitedContent::None`].
/// * [`ProhibitedContent::Unknown`] is **never** returned by this function –
///   it is the default before any analysis runs.
pub fn classify_by_text(title: &str, description: Option<&str>) -> ProhibitedContent {
    let text = match description {
        Some(desc) => format!("{title} {desc}").to_lowercase(),
        None => title.to_lowercase(),
    };

    let is_nazi = NAZI_KEYWORDS
        .iter()
        .any(|keyword| contains_keyword(&text, keyword));

    if is_nazi {
        ProhibitedContent::NaziGermany
    } else {
        ProhibitedContent::None
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =======================================================================
    // Safe / benign listings – must NOT be flagged
    // =======================================================================

    mod should_not_flag {
        use super::*;

        #[test]
        fn should_return_none_when_no_nazi_keywords() {
            assert_eq!(
                classify_by_text("Barocker Schrank aus dem 18. Jahrhundert", None),
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_return_none_when_empty_title() {
            assert_eq!(classify_by_text("", None), ProhibitedContent::None);
        }

        #[test]
        fn should_not_flag_when_keyword_is_substring_of_another_word() {
            // "nsdapping" contains "nsdap" as prefix but must not match
            assert_eq!(
                classify_by_text("Some nsdapping decoration", None),
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_not_flag_antique_furniture_listing() {
            assert_eq!(
                classify_by_text(
                    "Biedermeier Kommode 19. Jahrhundert Mahagoni",
                    Some("Sehr gut erhalten, aus südddeutschem Privatbesitz")
                ),
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_not_flag_iron_cross_ww1_listing() {
            // An Iron Cross can be WWI – no additional Nazi qualifier here
            assert_eq!(
                classify_by_text("Eisernes Kreuz 1914 Erster Weltkrieg", None),
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_not_flag_generic_eagle_sculpture() {
            assert_eq!(
                classify_by_text("Bronze Adlerfigur Jugendstil", None),
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_not_flag_general_ww2_book() {
            assert_eq!(
                classify_by_text(
                    "Der Zweite Weltkrieg – Geschichte und Ursachen",
                    Some("Umfassendes Werk über den Verlauf des Zweiten Weltkriegs")
                ),
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_not_flag_word_ending_with_keyword_fragment() {
            // plain "kreuz" alone must NOT flag
            assert_eq!(
                classify_by_text("Antikes Silberkreuz", None),
                ProhibitedContent::None
            );
        }

        #[test]
        fn should_not_flag_generic_military_uniform() {
            assert_eq!(
                classify_by_text("Uniform Bundeswehr 1970er Jahre", None),
                ProhibitedContent::None
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
                classify_by_text("Orden aus dem Dritten Reich", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_title_contains_dritte_reich_variant() {
            assert_eq!(
                classify_by_text("Abzeichen Dritte Reich 1940", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_title_contains_3_punkt_reich() {
            assert_eq!(
                classify_by_text("Medaille 3. Reich Silber Original", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_title_contains_iii_punkt_reich() {
            assert_eq!(
                classify_by_text("Feldmütze III. Reich WWII", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_description_contains_hakenkreuz() {
            assert_eq!(
                classify_by_text(
                    "Antike Medaille",
                    Some("Medaille mit Hakenkreuz-Symbol graviert")
                ),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_hakenkreuzfahne() {
            assert_eq!(
                classify_by_text("Hakenkreuzfahne original 1935", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_reichsadler() {
            assert_eq!(
                classify_by_text("Reichsadler Briefbeschwerer Bronze 1938", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nsdap_badge() {
            assert_eq!(
                classify_by_text("NSDAP Abzeichen 1935 Original", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_hitlerjugend_knife() {
            assert_eq!(
                classify_by_text("Hitlerjugend Messer mit Scheide 1935", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_schutzstaffel() {
            assert_eq!(
                classify_by_text("Schutzstaffel Dienstglas 8x30", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_sturmabteilung() {
            assert_eq!(
                classify_by_text("Sturmabteilung SA Dolch M1933", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_waffen_ss_with_hyphen() {
            assert_eq!(
                classify_by_text("Waffen-SS Feldmütze WWII Original", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_waffen_ss_without_hyphen() {
            assert_eq!(
                classify_by_text("Waffen SS Uniformjacke 1943", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_totenkopfring() {
            assert_eq!(
                classify_by_text("SS Totenkopfring Silber Replik", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_gestapo_badge() {
            assert_eq!(
                classify_by_text("Gestapo Dienstmarke original", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nationalsozialismus_in_description() {
            assert_eq!(
                classify_by_text(
                    "Antike Medaille Konvolut",
                    Some("Aus der Zeit des Nationalsozialismus, guter Zustand")
                ),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_bund_deutscher_maedel() {
            assert_eq!(
                classify_by_text("Bund Deutscher Mädel Abzeichen BDM", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_ss_rune() {
            assert_eq!(
                classify_by_text("SS-Rune Silber Anhänger Original", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_schwarze_sonne() {
            assert_eq!(
                classify_by_text("Schwarze Sonne Wandteller Keramik", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_adolf_hitler_portrait() {
            assert_eq!(
                classify_by_text("Ölgemälde Porträt Adolf Hitler signiert", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_heil_hitler_inscription() {
            assert_eq!(
                classify_by_text("Postkarte mit Aufschrift Heil Hitler 1937", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_heinrich_himmler_signature() {
            assert_eq!(
                classify_by_text("Autogramm Heinrich Himmler", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_reichsparteitag_photo() {
            assert_eq!(
                classify_by_text("Originalfoto Reichsparteitag Nürnberg 1937", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_leibstandarte() {
            assert_eq!(
                classify_by_text("Leibstandarte SS Adolf Hitler Manschettenknöpfe", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_kristallnacht_photo() {
            assert_eq!(
                classify_by_text("Originaldokument Reichskristallnacht November 1938", None),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Medal from the Third Reich 1939", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany() {
            assert_eq!(
                classify_by_text("Badge from Nazi Germany WWII", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_swastika() {
            assert_eq!(
                classify_by_text("Bronze pendant with Swastika symbol", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_medal_lot() {
            assert_eq!(
                classify_by_text("Nazi medal lot – original WWII collection", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_hitler_youth_dagger() {
            assert_eq!(
                classify_by_text("Hitler Youth Dagger HJ 1937 original", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_waffen_ss_helmet() {
            assert_eq!(
                classify_by_text("Waffen-SS Steel Helmet M42 WWII", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_ss_dagger() {
            assert_eq!(
                classify_by_text("SS Dagger Model 1936 with scabbard", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_armband() {
            assert_eq!(
                classify_by_text("Original Nazi armband with eagle insignia", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_national_socialism_in_description() {
            assert_eq!(
                classify_by_text(
                    "Historical artefact",
                    Some("Object related to National Socialism from 1938")
                ),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_gestapo_badge_english() {
            assert_eq!(
                classify_by_text("Gestapo Secret Police badge original WW2", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_case_insensitive_upper() {
            assert_eq!(
                classify_by_text("Medal from the THIRD REICH era", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_case_insensitive_mixed() {
            assert_eq!(
                classify_by_text("Rare Nazi Memorabilia Collection", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_reinhard_heydrich_portrait() {
            assert_eq!(
                classify_by_text("Portrait photograph Reinhard Heydrich", None),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Médaille du Troisième Reich 1939", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_croix_gammee() {
            assert_eq!(
                classify_by_text("Médaille avec croix gammée originale", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_allemagne_nazie() {
            assert_eq!(
                classify_by_text("Objet de l'Allemagne nazie seconde guerre", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_national_socialisme() {
            assert_eq!(
                classify_by_text(
                    "Document historique",
                    Some("Lié au national-socialisme allemand")
                ),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Medalla del Tercer Reich Segunda Guerra", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_cruz_gamada() {
            assert_eq!(
                classify_by_text("Colgante de plata con cruz gamada", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_alemania_nazi() {
            assert_eq!(
                classify_by_text("Insignia de la Alemania Nazi WWII", None),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Medaglia del Terzo Reich 1940", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_svastica() {
            assert_eq!(
                classify_by_text("Pendente argento con svastica originale", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_germania_nazista() {
            assert_eq!(
                classify_by_text("Oggetto della Germania Nazista seconda guerra", None),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Medaille Derde Rijk WOII origineel", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_hakenkruis() {
            assert_eq!(
                classify_by_text("Zilveren hanger met hakenkruis 1938", None),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Medalha do Terceiro Reich 1939", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_suastica() {
            assert_eq!(
                classify_by_text("Pendente prata com suástica original", None),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Medal Trzecia Rzesza II Wojna Światowa", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_swastyka() {
            assert_eq!(
                classify_by_text("Rzadka swastyka z 1938 oryginał", None),
                ProhibitedContent::NaziGermany
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
                classify_by_text("Antike Uniform", Some("Schutzstaffel Dienstuniform")),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_keyword_at_start_of_text() {
            assert_eq!(
                classify_by_text("Hakenkreuz Wanddeko", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_keyword_at_end_of_text() {
            assert_eq!(
                classify_by_text("Silbernes Abzeichen Drittes Reich", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_multiple_keywords_present() {
            assert_eq!(
                classify_by_text("NSDAP Abzeichen Waffen-SS Orden Hakenkreuz", None),
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_return_none_when_description_is_none_and_title_is_benign() {
            assert_eq!(
                classify_by_text("Völlig harmloses Gemälde", None),
                ProhibitedContent::None
            );
        }
    }
}
