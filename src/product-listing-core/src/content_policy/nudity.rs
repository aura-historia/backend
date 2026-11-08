use super::keywords::KeywordSet;

/// Terms that indicate nudity, erotica or pornography in listing text, in the catalogue
/// languages and the further European languages listing sources write in. Every keyword is
/// lowercase.
///
/// Nude art is the common case in antiques, so genre terms ("frauenakt", "nu couché", "裸婦")
/// count as much as explicit ones. Words that are also common in another sense stay out or only
/// appear inside phrases: German, Polish and Czech "akt" (act, deed, file), Spanish "nudo"
/// (knot, as in hand-knotted rugs), French "nu" and Dutch "nu" (now), Swedish "sex" (six),
/// "naga" (the serpent deity in Asian art), a bare "裸" (as in "裸钻", a loose diamond) and bare
/// "色情"/"情色" (which span word boundaries, as in "玫瑰金色情侣表"). Exclusions blank out set
/// phrases that use a keyword in another sense, such as "naked eye" or the "长春宫" reign mark.
pub(super) const NUDITY_KEYWORDS: KeywordSet = KeywordSet {
    words: &[
        // ── ENGLISH ───────────────────────────────────────────────────────
        "nude",
        "nudes",
        "seminude",
        "naked",
        "topless",
        "bare-breasted",
        "bare breasted",
        "porn",
        "porno",
        "pornos",
        "x-rated",
        "nsfw",
        "hentai",
        "shunga",
        "kamasutra",
        "kama sutra",
        "striptease",
        "playboy",
        "hustler magazine",
        "hustler magazin",
        // ── GERMAN ── "akt" only in compounds and phrases ─────────────────
        "damenakt",
        "damenakte",
        "damenakts",
        "damenaktes",
        "mädchenakt",
        "mädchenakte",
        "mädchenakts",
        "mädchenaktes",
        "männerakt",
        "männerakte",
        "männerakts",
        "männeraktes",
        "jünglingsakt",
        "jünglingsakts",
        "jünglingsaktes",
        "knabenakt",
        "knabenakts",
        "knabenaktes",
        "akt einer frau",
        "akt eines mannes",
        "akt im",
        "akt am",
        "akt mit tuch",
        "akt, öl",
        "splitternackt",
        // ── FRENCH ── "nu" only in phrases ────────────────────────────────
        "nu féminin",
        "nu masculin",
        "nu couché",
        "nu allongé",
        "nu assis",
        "nu debout",
        "nu de dos",
        "nu de femme",
        "nu d'homme",
        "nu académique",
        "nu artistique",
        "étude de nu",
        "femme nue",
        "femmes nues",
        "jeune fille nue",
        "homme nu",
        "hommes nus",
        "seins nus",
        "baigneuse nue",
        "baigneuses nues",
        "nus féminins",
        "nus masculins",
        "jeune femme nue",
        "nu, huile",
        // ── ITALIAN ── "nudo" only in phrases ─────────────────────────────
        "nudo femminile",
        "nudo maschile",
        "nudo di donna",
        "nudo d'uomo",
        "nudo disteso",
        "nudo sdraiato",
        "nudo seduto",
        "nudo in piedi",
        "nudo artistico",
        "nudo accademico",
        "studio di nudo",
        "nudo di schiena",
        "nudo di ragazza",
        "nudo, olio",
        "nudi femminili",
        "nudi maschili",
        "donna nuda",
        "figura nuda",
        // ── PORTUGUESE ────────────────────────────────────────────────────
        "mulher nua",
        "mulheres nuas",
        "figura nua",
        "nudez",
        "nu artístico",
        "nu feminino",
        "nu masculino",
        // ── SWEDISH / NORWEGIAN / DANISH ──────────────────────────────────
        "nakna",
        "naket",
        "nakne",
        "nøgne",
        // ── HUNGARIAN ─────────────────────────────────────────────────────
        "meztelen",
        "női akt",
        "férfi akt",
        // ── POLISH ────────────────────────────────────────────────────────
        "nagi",
        "nagie",
        "nagość",
        "naga kobieta",
        "nagi mężczyzna",
        "akt kobiecy",
        "akt kobiety",
        "akt męski",
        "akt mężczyzny",
        // ── CZECH / SLOVAK ────────────────────────────────────────────────
        "nahá",
        "nahý",
        "nahé",
        "nahí",
        "nahota",
        "nahotu",
        "ženský akt",
        "mužský akt",
        "akt ženy",
        "akt muže",
        "ženski akt",
        "muški akt",
        // ── GREEK ─────────────────────────────────────────────────────────
        "γυμνή",
        "γυμνό",
        "γυμνές",
        // ── RUSSIAN ───────────────────────────────────────────────────────
        "ню",
        "голая",
        "голые",
        "голый",
        "нагая",
        "нагие",
        "нагота",
        "наготы",
        "порно",
        // ── TURKISH ───────────────────────────────────────────────────────
        "çıplak",
        "çiplak",
        "nü",
        "müstehcen",
        // ── ARABIC ────────────────────────────────────────────────────────
        "عارية",
        "العارية",
        "عاريات",
        "عاري",
        "العاري",
        "تعري",
        "التعري",
        "إباحي",
        "إباحية",
        "الإباحية",
        "اباحي",
        "اباحية",
        "الاباحية",
        "إيروتيكي",
        "إيروتيكية",
    ],
    word_prefixes: &[
        // ── SHARED STEMS ── erotic, erotica, erotisch, erotico, erotique,
        // erotisk, erotikus, pornography, pornografie, pornográfico, …
        "eroti",
        "éroti",
        "erótic",
        "eroty",
        "eroottin",
        "pornogr",
        "nudit",
        "nudis",
        "naturis",
        // ── GERMAN ────────────────────────────────────────────────────────
        "nackt",
        "halbnackt",
        "frauenakt",
        "rückenakt",
        "halbakt",
        "weibliche akt",
        "weiblicher akt",
        "weiblichen akt",
        "männliche akt",
        "männlicher akt",
        "männlichen akt",
        "liegende akt",
        "liegender akt",
        "liegenden akt",
        "sitzende akt",
        "sitzender akt",
        "sitzenden akt",
        "stehende akt",
        "stehender akt",
        "stehenden akt",
        "kniende akt",
        "kniender akt",
        "knienden akt",
        "akt-studi",
        "akt-zeichnung",
        "aktstudi",
        "aktzeichnung",
        "aktbild",
        "aktfoto",
        "aktgemälde",
        "aktdarstellung",
        "aktskulptur",
        "aktfigur",
        "aktplastik",
        "aktmalerei",
        "aktmodell",
        "aktportr",
        "aktaufnahme",
        "aktpostkarte",
        "freikörperkultur",
        // ── SPANISH / PORTUGUESE ──────────────────────────────────────────
        "desnud",
        "semidesnud",
        // ── DUTCH ─────────────────────────────────────────────────────────
        "naakt",
        "halfnaakt",
        // ── SWEDISH / NORWEGIAN / DANISH / FINNISH ────────────────────────
        "naken",
        "nøgen",
        "alaston",
        // ── RUSSIAN ───────────────────────────────────────────────────────
        "обнажённ",
        "обнаженн",
        "эроти",
        "порногр",
        // ── GREEK ─────────────────────────────────────────────────────────
        "πορνογρ",
    ],
    substrings: &[
        // ── CHINESE ───────────────────────────────────────────────────────
        "裸体",
        "裸體",
        "裸女",
        "裸男",
        "裸照",
        "人体艺术",
        "人體藝術",
        "人体油画",
        "人體油畫",
        "人体素描",
        "人體素描",
        "人体摄影",
        "人體攝影",
        "色情图",
        "色情圖",
        "色情画",
        "色情畫",
        "色情照",
        "色情片",
        "色情杂志",
        "色情雜誌",
        "情色艺术",
        "情色藝術",
        "情色摄影",
        "情色攝影",
        "春宫图",
        "春宫画",
        "春宮圖",
        "春宮畫",
        "春画",
        "春畫",
        "秘戏图",
        "秘戲圖",
        // ── JAPANESE ──────────────────────────────────────────────────────
        "ヌード",
        "裸婦",
        "枕絵",
        "艶本",
        "エロティック",
        "エロチック",
        "ポルノ",
        "わいせつ",
        "猥褻",
        // ── KOREAN ────────────────────────────────────────────────────────
        "누드",
    ],
    exclusions: &[
        "naked eye",
        "naked-eye",
        "naked lunch",
        "naked and the dead",
        "naked ape",
        "playboy of the western world",
        "nude-colo",
        "farbe nude",
        "farbe: nude",
        "color: nude",
        "colour: nude",
        "nackte holz",
        "nackten holz",
        "nacktes holz",
        "frauenaktion",
        "halbaktiv",
        "shi nü",
        "shi-nü",
        "ヌードル",
        "早春画",
        "早春畫",
        "新春画",
        "新春畫",
        "迎春画",
        "迎春畫",
        "游春画",
        "遊春畫",
    ],
};

#[cfg(test)]
mod tests {
    use crate::content_policy::{
        ContentPolicyDecision, SensitiveContentCategory, assess_listing_text,
    };

    const NUDITY: Option<ContentPolicyDecision> = Some(ContentPolicyDecision::RequiresConsent(
        SensitiveContentCategory::Nudity,
    ));
    const ALLOWED: Option<ContentPolicyDecision> = Some(ContentPolicyDecision::Allowed);

    #[rstest::rstest]
    // Reported listings (#2830)
    #[case("Mercedes Frauenakt", None)]
    #[case(
        "Anonym, Folge von 6 photographischen Reproduktionen von erotischen bzw. pornographischen Darstellungen",
        None
    )]
    // English
    #[case("Reclining Female Nude, oil on canvas", None)]
    #[case("Naked bather, bronze", None)]
    #[case("Vintage erotica photograph", None)]
    #[case("Erotic etching, 19th century", None)]
    #[case("Pornographic postcard", None)]
    #[case("Topless pin-up calendar 1955", None)]
    #[case("Japanese shunga woodblock print", None)]
    #[case("Semi-nude dancer figurine", None)]
    // German
    #[case("Aktstudie, Kohle auf Papier", None)]
    #[case("Weiblicher Akt am Strand", None)]
    #[case("Nackte Frau am Brunnen", None)]
    #[case("Aktfotografie der 1920er Jahre", None)]
    #[case("Halbakt, Öl auf Leinwand", None)]
    #[case("Erotische Postkarte um 1900", None)]
    #[case("FKK Magazin", Some("Zeitschrift der Freikörperkultur"))]
    // French
    #[case("Nu couché, huile sur toile", None)]
    #[case("Femme nue au miroir", None)]
    #[case("Gravure érotique", None)]
    // Spanish
    #[case("Mujer desnuda, óleo", None)]
    #[case("Grabado erótico", None)]
    // Italian
    #[case("Nudo femminile, olio su tela", None)]
    #[case("Stampa erotica", None)]
    // Portuguese
    #[case("Mulher nua, pintura", None)]
    #[case("Fotografia erótica", None)]
    // Dutch
    #[case("Naaktstudie van een vrouw", None)]
    #[case("Erotische prent", None)]
    // Polish
    #[case("Akt kobiecy, olej", None)]
    #[case("Fotografia erotyczna", None)]
    // Czech
    #[case("Ženský akt, olej na plátně", None)]
    #[case("Nahá žena u řeky", None)]
    // Scandinavian, Finnish, Hungarian
    #[case("Naken kvinna, olja på duk", None)]
    #[case("Nøgen kvinde", None)]
    #[case("Alastonkuva", None)]
    #[case("Meztelen nő", None)]
    // Russian
    #[case("Обнажённая натурщица, холст, масло", None)]
    #[case("Картина в стиле ню", None)]
    #[case("Эротическая открытка", None)]
    // Turkish
    #[case("Çıplak kadın, yağlı boya", None)]
    // Arabic
    #[case("لوحة امرأة عارية", None)]
    // Chinese
    #[case("清代春宫图册", None)]
    #[case("民国裸女照片", None)]
    // Japanese
    #[case("裸婦像 油彩", None)]
    #[case("ヌード写真集", None)]
    #[case("江戸時代の春画", None)]
    // Inflections, compounds and magazines
    #[case("Darstellung eines weiblichen Aktes", None)]
    #[case("Liegende Akte, Radierungen", None)]
    #[case("Akt-Studie, Rötel", None)]
    #[case("Frauenaktbild in Goldrahmen", None)]
    #[case("Akt mit Tuch, Bronze", None)]
    #[case("Nudo di schiena, olio su tela", None)]
    #[case("Baigneuse nue, bronze", None)]
    #[case("Nu artístico, fotografia", None)]
    #[case("Playboy Magazine December 1953", None)]
    #[case("ÇIPLAK KADIN, yağlı boya", None)]
    #[case("民国人体油画", None)]
    #[case("日本昭和nude写真集", None)]
    #[case("누드 사진집", None)]
    #[case("Ελαιογραφία, γυμνή γυναίκα", None)]
    fn should_flag_nudity(#[case] title: &str, #[case] description: Option<&str>) {
        assert_eq!(assess_listing_text(Some(title), description), NUDITY);
    }

    #[test]
    fn should_flag_nudity_in_the_description_only() {
        assert_eq!(
            assess_listing_text(
                Some("Ölgemälde, 19. Jahrhundert"),
                Some("Darstellung eines liegenden Frauenakts. Signiert.")
            ),
            NUDITY
        );
    }

    #[rstest::rstest]
    #[case("Oper in drei Akten, Libretto 1890")]
    #[case("Historische Aktie der Deutschen Bank")]
    #[case("Akte des Landgerichts, 1910")]
    #[case("Nudelholz aus Buchenholz")]
    #[case("Nackenrolle, Biedermeier")]
    #[case("Alfombra persa, nudo a mano")]
    #[case("Akt notarialny z 1920 roku")]
    #[case("Nu verkrijgbaar: antieke kast")]
    #[case("Sex stolar i ek, 1800-tal")]
    #[case("Essex oak chest")]
    #[case("Bronze Naga Buddha, Thailand")]
    #[case("Меню ресторана, 1913")]
    #[case("天然裸钻 1克拉")]
    #[case("Heroic bronze statue of Eros")]
    #[case("Naturalistic landscape painting")]
    #[case("Tête de femme, pastel")]
    #[case("Tiny inclusion, not visible to the naked eye")]
    #[case("Naked Lunch, William Burroughs, first edition")]
    #[case("The Playboy of the Western World, Synge, 1907")]
    #[case("Handtasche Leder, Farbe nude")]
    #[case("Nudie Jeans, Grim Tim")]
    #[case("Kommode, bis aufs nackte Holz abgelaugt")]
    #[case("Plakat der Frauenaktion 1975")]
    #[case("Famille rose vase painted with shi nü")]
    #[case("光绪 长春宫制 款 粉彩盘")]
    #[case("玫瑰金色情侣对表")]
    #[case("民族风情色彩浓厚的刺绣")]
    #[case("日清カップヌードル 1971 レトロ")]
    #[case("早春画卷 立轴")]
    fn should_not_flag_lookalikes(#[case] title: &str) {
        assert_eq!(assess_listing_text(Some(title), None), ALLOWED);
    }
}
