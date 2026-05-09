use crate::core::prohibited_content::ProhibitedContent;
use crate::service::product_command::CreateProductCommand;

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

    fn make_images(n: usize) -> Vec<ProductImage> {
        fake::vec![ProductImage; n]
    }

    // =======================================================================
    // Safe / benign listings – must NOT be flagged
    // =======================================================================

    mod should_not_flag {
        use super::*;

        #[test]
        fn should_set_none_when_no_nazi_keywords() {
            let mut cmd = cmd_with("Barocker Schrank aus dem 18. Jahrhundert", None);
            cmd.images = make_images(2);
            classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::None);
            }
        }

        #[test]
        fn should_set_none_when_no_images() {
            let mut cmd = cmd_with("Drittes Reich Medaille", None);
            cmd.images = vec![];
            classify_images(&mut cmd);
            assert!(cmd.images.is_empty());
        }

        #[test]
        fn should_not_flag_when_keyword_is_substring_of_another_word() {
            // "nsdapping" contains "nsdap" as prefix but must not match
            let mut cmd = cmd_with("Some nsdapping decoration", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(cmd.images[0].prohibited_content, ProhibitedContent::None);
        }

        #[test]
        fn should_not_flag_antique_furniture_listing() {
            let mut cmd = cmd_with(
                "Biedermeier Kommode 19. Jahrhundert Mahagoni",
                Some("Sehr gut erhalten, aus südddeutschem Privatbesitz"),
            );
            cmd.images = make_images(2);
            classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::None);
            }
        }

        #[test]
        fn should_not_flag_iron_cross_ww1_listing() {
            // An Iron Cross can be WWI – no additional Nazi qualifier here
            let mut cmd = cmd_with("Eisernes Kreuz 1914 Erster Weltkrieg", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(cmd.images[0].prohibited_content, ProhibitedContent::None);
        }

        #[test]
        fn should_not_flag_generic_eagle_sculpture() {
            let mut cmd = cmd_with("Bronze Adlerfigur Jugendstil", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(cmd.images[0].prohibited_content, ProhibitedContent::None);
        }

        #[test]
        fn should_not_flag_general_ww2_book() {
            let mut cmd = cmd_with(
                "Der Zweite Weltkrieg – Geschichte und Ursachen",
                Some("Umfassendes Werk über den Verlauf des Zweiten Weltkriegs"),
            );
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(cmd.images[0].prohibited_content, ProhibitedContent::None);
        }

        #[test]
        fn should_not_flag_word_ending_with_keyword_fragment() {
            // "hakenkreuzförmig" starts with "hakenkreuz" – must still flag
            // but plain "kreuz" alone must NOT flag
            let mut cmd = cmd_with("Antikes Silberkreuz", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(cmd.images[0].prohibited_content, ProhibitedContent::None);
        }

        #[test]
        fn should_not_flag_generic_military_uniform() {
            let mut cmd = cmd_with("Uniform Bundeswehr 1970er Jahre", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(cmd.images[0].prohibited_content, ProhibitedContent::None);
        }
    }

    // =======================================================================
    // German-language Nazi listings – MUST be flagged
    // =======================================================================

    mod german {
        use super::*;

        #[test]
        fn should_flag_when_title_contains_drittes_reich() {
            let mut cmd = cmd_with("Orden aus dem Dritten Reich", None);
            cmd.images = make_images(3);
            classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::NaziGermany);
            }
        }

        #[test]
        fn should_flag_when_title_contains_dritte_reich_variant() {
            let mut cmd = cmd_with("Abzeichen Dritte Reich 1940", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_title_contains_3_punkt_reich() {
            let mut cmd = cmd_with("Medaille 3. Reich Silber Original", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_title_contains_iii_punkt_reich() {
            let mut cmd = cmd_with("Feldmütze III. Reich WWII", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_description_contains_hakenkreuz() {
            let mut cmd = cmd_with(
                "Antike Medaille",
                Some("Medaille mit Hakenkreuz-Symbol graviert"),
            );
            cmd.images = make_images(2);
            classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::NaziGermany);
            }
        }

        #[test]
        fn should_flag_hakenkreuzfahne() {
            let mut cmd = cmd_with("Hakenkreuzfahne original 1935", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_reichsadler() {
            let mut cmd = cmd_with("Reichsadler Briefbeschwerer Bronze 1938", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nsdap_badge() {
            let mut cmd = cmd_with("NSDAP Abzeichen 1935 Original", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_hitlerjugend_knife() {
            let mut cmd = cmd_with("Hitlerjugend Messer mit Scheide 1935", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_schutzstaffel() {
            let mut cmd = cmd_with("Schutzstaffel Dienstglas 8x30", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_sturmabteilung() {
            let mut cmd = cmd_with("Sturmabteilung SA Dolch M1933", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_waffen_ss_with_hyphen() {
            let mut cmd = cmd_with("Waffen-SS Feldmütze WWII Original", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_waffen_ss_without_hyphen() {
            let mut cmd = cmd_with("Waffen SS Uniformjacke 1943", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_totenkopfring() {
            let mut cmd = cmd_with("SS Totenkopfring Silber Replik", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_gestapo_badge() {
            let mut cmd = cmd_with("Gestapo Dienstmarke original", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nationalsozialismus_in_description() {
            let mut cmd = cmd_with(
                "Antike Medaille Konvolut",
                Some("Aus der Zeit des Nationalsozialismus, guter Zustand"),
            );
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_bund_deutscher_maedel() {
            let mut cmd = cmd_with("Bund Deutscher Mädel Abzeichen BDM", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_ss_rune() {
            let mut cmd = cmd_with("SS-Rune Silber Anhänger Original", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_schwarze_sonne() {
            let mut cmd = cmd_with("Schwarze Sonne Wandteller Keramik", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_adolf_hitler_portrait() {
            let mut cmd = cmd_with("Ölgemälde Porträt Adolf Hitler signiert", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_heil_hitler_inscription() {
            let mut cmd = cmd_with("Postkarte mit Aufschrift Heil Hitler 1937", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_heinrich_himmler_signature() {
            let mut cmd = cmd_with("Autogramm Heinrich Himmler", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_reichsparteitag_photo() {
            let mut cmd = cmd_with("Originalfoto Reichsparteitag Nürnberg 1937", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_leibstandarte() {
            let mut cmd = cmd_with("Leibstandarte SS Adolf Hitler Manschettenknöpfe", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_kristallnacht_photo() {
            let mut cmd = cmd_with("Originaldokument Reichskristallnacht November 1938", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
            let mut cmd = cmd_with("Medal from the Third Reich 1939", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_germany() {
            let mut cmd = cmd_with("Badge from Nazi Germany WWII", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_swastika() {
            let mut cmd = cmd_with("Bronze pendant with Swastika symbol", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_medal_lot() {
            let mut cmd = cmd_with("Nazi medal lot – original WWII collection", None);
            cmd.images = make_images(2);
            classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::NaziGermany);
            }
        }

        #[test]
        fn should_flag_hitler_youth_dagger() {
            let mut cmd = cmd_with("Hitler Youth Dagger HJ 1937 original", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_waffen_ss_helmet() {
            let mut cmd = cmd_with("Waffen-SS Steel Helmet M42 WWII", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_ss_dagger() {
            let mut cmd = cmd_with("SS Dagger Model 1936 with scabbard", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_nazi_armband() {
            let mut cmd = cmd_with("Original Nazi armband with eagle insignia", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_national_socialism_in_description() {
            let mut cmd = cmd_with(
                "Historical artefact",
                Some("Object related to National Socialism from 1938"),
            );
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_gestapo_badge_english() {
            let mut cmd = cmd_with("Gestapo Secret Police badge original WW2", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_case_insensitive_upper() {
            let mut cmd = cmd_with("Medal from the THIRD REICH era", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_case_insensitive_mixed() {
            let mut cmd = cmd_with("Rare Nazi Memorabilia Collection", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_reinhard_heydrich_portrait() {
            let mut cmd = cmd_with("Portrait photograph Reinhard Heydrich", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
            let mut cmd = cmd_with("Médaille du Troisième Reich 1939", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_croix_gammee() {
            let mut cmd = cmd_with("Médaille avec croix gammée originale", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_allemagne_nazie() {
            let mut cmd = cmd_with("Objet de l'Allemagne nazie seconde guerre", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_national_socialisme() {
            let mut cmd = cmd_with(
                "Document historique",
                Some("Lié au national-socialisme allemand"),
            );
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
            let mut cmd = cmd_with("Medalla del Tercer Reich Segunda Guerra", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_cruz_gamada() {
            let mut cmd = cmd_with("Colgante de plata con cruz gamada", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_alemania_nazi() {
            let mut cmd = cmd_with("Insignia de la Alemania Nazi WWII", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
            let mut cmd = cmd_with("Medaglia del Terzo Reich 1940", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_svastica() {
            let mut cmd = cmd_with("Pendente argento con svastica originale", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_germania_nazista() {
            let mut cmd = cmd_with("Oggetto della Germania Nazista seconda guerra", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
            let mut cmd = cmd_with("Medaille Derde Rijk WOII origineel", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_hakenkruis() {
            let mut cmd = cmd_with("Zilveren hanger met hakenkruis 1938", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
            let mut cmd = cmd_with("Medalha do Terceiro Reich 1939", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_suastica() {
            let mut cmd = cmd_with("Pendente prata com suástica original", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
            let mut cmd = cmd_with("Medal Trzecia Rzesza II Wojna Światowa", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_swastyka() {
            let mut cmd = cmd_with("Rzadka swastyka z 1938 oryginał", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
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
        fn should_flag_all_images_when_multi_image_listing_has_nazi_term() {
            let mut cmd = cmd_with("NSDAP Lot mit mehreren Orden", None);
            cmd.images = make_images(5);
            classify_images(&mut cmd);
            assert!(
                cmd.images
                    .iter()
                    .all(|img| img.prohibited_content == ProhibitedContent::NaziGermany)
            );
        }

        #[test]
        fn should_flag_when_keyword_only_in_description() {
            let mut cmd = cmd_with("Antike Uniform", Some("Schutzstaffel Dienstuniform"));
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_keyword_at_start_of_text() {
            let mut cmd = cmd_with("Hakenkreuz Wanddeko", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_keyword_at_end_of_text() {
            let mut cmd = cmd_with("Silbernes Abzeichen Drittes Reich", None);
            cmd.images = make_images(1);
            classify_images(&mut cmd);
            assert_eq!(
                cmd.images[0].prohibited_content,
                ProhibitedContent::NaziGermany
            );
        }

        #[test]
        fn should_flag_when_multiple_keywords_present() {
            let mut cmd = cmd_with("NSDAP Abzeichen Waffen-SS Orden Hakenkreuz", None);
            cmd.images = make_images(2);
            classify_images(&mut cmd);
            for img in &cmd.images {
                assert_eq!(img.prohibited_content, ProhibitedContent::NaziGermany);
            }
        }

        #[test]
        fn should_set_none_for_listing_with_zero_images() {
            let mut cmd = cmd_with("Völlig harmloses Gemälde", None);
            cmd.images = vec![];
            classify_images(&mut cmd);
            assert!(cmd.images.is_empty());
        }
    }
}
