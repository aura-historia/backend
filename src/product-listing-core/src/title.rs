use domain_primitives::string_newtype;
use unicode_normalization::UnicodeNormalization;

mod typography;

string_newtype!(
    Title,
    struct_only,
    derives(serde::Serialize, serde::Deserialize)
);

impl Title {
    pub const MAX_CHARS: usize = 128;

    /// Returns whether `text` satisfies the structural title rules: trimmed, without a
    /// sentence-final period, capitalized and at most [`Self::MAX_CHARS`] characters.
    ///
    /// Every constructed title does, and so do titles stored before typography normalization.
    /// Readers of stored or in-flight titles accept such values and present `Title::from(text)`.
    pub fn has_canonical_structure(text: &str) -> bool {
        truncate(capitalize(strip_sentence_periods(text))) == text
    }
}

impl From<&str> for Title {
    fn from(s: &str) -> Self {
        let text = typography::normalize(s);
        // Capitalization can decompose a letter, as for `ΐ`; recompose before truncating.
        let capitalized: String = capitalize(strip_sentence_periods(&text)).nfc().collect();
        Title(truncate(capitalized))
    }
}

impl From<String> for Title {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<&String> for Title {
    fn from(s: &String) -> Self {
        Self::from(s.as_str())
    }
}

fn strip_sentence_periods(s: &str) -> &str {
    let mut s = s.trim();
    // Keep ellipses, including shortened ellipses in previously persisted titles.
    // Removing sentence punctuation can expose whitespace or another sentence period.
    while !s.ends_with("..") {
        let Some(without_period) = s.strip_suffix('.') else {
            break;
        };
        s = without_period.trim_end();
    }
    s
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let first_upper: String = first.to_uppercase().collect();
            first_upper + chars.as_str()
        }
    }
}

fn truncate(s: String) -> String {
    const ELLIPSIS: &str = "...";
    const ELLIPSIS_CHAR_LEN: usize = 3;

    if s.chars().count() > Title::MAX_CHARS {
        let truncated: String = s
            .chars()
            .take(Title::MAX_CHARS - ELLIPSIS_CHAR_LEN)
            .collect();
        truncated + ELLIPSIS
    } else {
        s
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for Title {
    fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
        use fake::Fake;
        let paragraphs: Vec<String> = fake::faker::lorem::en::Words(2..10).fake_with_rng(rng);
        Self(paragraphs.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use crate::title::Title;

    #[cfg(feature = "test-data")]
    #[test]
    fn should_fake_title() {
        use fake::{Fake, Faker};

        let _ = Faker.fake::<Title>();
    }

    #[test]
    fn should_trim_title() {
        let title = Title::from("   Hello World   ");
        assert_eq!(title.as_ref(), "Hello World");
    }

    #[test]
    fn should_capitalize_first_letter() {
        let title = Title::from("hello world");
        assert_eq!(title.as_ref(), "Hello world");
    }

    #[test]
    fn should_truncate_long_title_to_max_length() {
        let long_title = "a".repeat(200);
        let title = Title::from(long_title.as_str());
        assert_eq!(title.as_ref().len(), 128);
        assert!(title.as_ref().ends_with("..."));
    }

    #[test]
    fn should_append_ellipsis_when_truncating_title() {
        let long_title = "a".repeat(200);
        let title = Title::from(long_title.as_str());
        // first 'a' is capitalised to 'A', then 124 lowercase 'a's, then "..."
        let expected = format!("A{}...", "a".repeat(124));
        assert_eq!(title.as_ref(), expected);
    }

    #[test]
    fn should_not_append_ellipsis_when_within_limit_for_title() {
        let short_title = "a".repeat(128);
        let title = Title::from(short_title.as_str());
        assert!(!title.as_ref().ends_with("..."));
        assert_eq!(title.as_ref().chars().count(), 128);
    }

    #[test]
    fn should_handle_empty_title() {
        let title = Title::from("");
        assert_eq!(title.as_ref(), "");
    }

    #[test]
    fn should_handle_title_with_period() {
        let title = Title::from("Hello World.");
        assert_eq!(title.as_ref(), "Hello World");
    }

    #[test]
    fn should_preserve_ellipsis_on_repeated_normalization() {
        for raw in [
            "a".repeat(200),
            "ä".repeat(200),
            "Title...".to_owned(),
            "Title..".to_owned(),
        ] {
            let title = Title::from(raw.as_str());
            assert_eq!(title, Title::from(title.as_ref()));
        }
    }

    #[test]
    fn should_trim_whitespace_exposed_by_terminal_periods() {
        for raw in [
            "hello world .",
            "hello world . .",
            "hello world\t.",
            "hello world... .",
        ] {
            let title = Title::from(raw);
            assert_eq!(title.as_ref(), title.as_ref().trim());
            assert_eq!(title, Title::from(title.as_ref()));
        }
        assert_eq!(Title::from("hello world .").as_ref(), "Hello world");
        assert_eq!(Title::from("hello world... .").as_ref(), "Hello world...");
    }

    #[test]
    fn should_capitalize_after_stripping_period() {
        let title = Title::from("hello world.");
        assert_eq!(title.as_ref(), "Hello world");
    }

    #[test]
    fn should_convert_to_string() {
        let title = Title::from("Antique vase");
        let s: String = title.into();
        assert_eq!(s, "Antique vase");
    }

    #[test]
    fn should_create_from_owned_string() {
        let title = Title::from("Antique vase".to_string());
        assert_eq!(title.as_ref(), "Antique vase");
    }

    #[test]
    fn should_display_title() {
        let title = Title::from("Antique vase");
        assert_eq!(format!("{title}"), "Antique vase");
    }

    #[test]
    fn should_normalize_decorative_staging_title() {
        let title = Title::from(
            "𝐄𝐢𝐧 𝐌ö𝐛𝐞𝐥, 𝐝𝐚𝐬 𝐑𝐮𝐡𝐞 𝐢𝐧 𝐃𝐞𝐢𝐧 𝐙𝐮𝐡𝐚𝐮𝐬𝐞 𝐛𝐫𝐢𝐧𝐠𝐭 🌿✨Anrichte Biedermeier Kirschbaum | 3360",
        );
        assert_eq!(
            title.as_ref(),
            "Ein Möbel, das Ruhe in Dein Zuhause bringt Anrichte Biedermeier Kirschbaum | 3360"
        );
    }

    #[test]
    fn should_normalize_mixed_ordinary_and_styled_text() {
        let title = Title::from("𝒃𝒊𝒆𝒅𝒆𝒓𝒎𝒆𝒊𝒆𝒓 Kommode 𝟏𝟖𝟑𝟎, Ｎｕｓｓｂａｕｍ ⓜⓐⓢⓢⓘⓥ");
        assert_eq!(title.as_ref(), "Biedermeier Kommode 1830, Nussbaum massiv");
    }

    #[test]
    fn should_capitalize_and_strip_periods_exposed_by_removed_pictographs() {
        assert_eq!(Title::from("✨ vase ✨").as_ref(), "Vase");
        assert_eq!(Title::from("🌿 vase. 🌿").as_ref(), "Vase");
        assert_eq!(Title::from("🌿🌿").as_ref(), "");
    }

    #[test]
    fn should_preserve_accented_text_and_identifiers() {
        let text = "Ölgemälde „Straße in Köln“, Größe 50×70 cm – Nr. 3360 (ca. 1890)";
        assert_eq!(Title::from(text).as_ref(), text);
    }

    #[test]
    fn should_compose_decomposed_accents() {
        assert_eq!(Title::from("Mo\u{308}bel").as_ref(), "Möbel");
        assert_eq!(
            Title::from("\u{3B9}\u{308}\u{301}ταλια").as_ref(),
            "\u{3AA}\u{301}ταλια"
        );
    }

    #[test]
    fn should_truncate_after_typography_normalization() {
        let title = Title::from(format!("🌿{}🌿", "𝐚".repeat(200)));
        assert_eq!(title.as_ref(), format!("A{}...", "a".repeat(124)));
    }

    #[test]
    fn should_normalize_idempotently() {
        for raw in [
            "𝐄𝐢𝐧 𝐌ö𝐛𝐞𝐥, 𝐝𝐚𝐬 𝐑𝐮𝐡𝐞 𝐢𝐧 𝐃𝐞𝐢𝐧 𝐙𝐮𝐡𝐚𝐮𝐬𝐞 𝐛𝐫𝐢𝐧𝐠𝐭 🌿✨Anrichte Biedermeier Kirschbaum | 3360",
            "Vase🌿Kommode",
            "🔥 | SALE | 🔥 vase",
            "ⓂⒺⒾⓈⓈⒺⓃ 🅰\u{FE0F} 1\u{FE0F}\u{20E3}",
            "Ölgemälde Größe ß",
            "  vase  \u{A0} rot.  ",
        ] {
            let title = Title::from(raw);
            assert_eq!(Title::from(title.as_ref()), title, "{raw:?}");
            assert!(Title::has_canonical_structure(title.as_ref()), "{raw:?}");
        }
    }

    #[test]
    fn should_construct_canonical_titles_for_every_character() {
        let samples = (0..=u32::from(char::MAX))
            .filter_map(char::from_u32)
            .filter(|c| {
                let code = u32::from(*c);
                code < 0x3000
                    || (0xFE00..=0xFFFF).contains(&code)
                    || (0x1D400..=0x1FFFF).contains(&code)
                    || (0xE0000..=0xE007F).contains(&code)
                    || c.to_uppercase().ne(std::iter::once(*c))
            });
        for c in samples {
            for raw in [
                format!("{c}"),
                format!("{c}a"),
                format!("{c}\u{301}a"),
                format!("a{c}"),
                format!("a {c} b"),
            ] {
                let title = Title::from(raw.as_str());
                assert_eq!(Title::from(title.as_ref()), title, "{raw:?}");
                assert!(Title::has_canonical_structure(title.as_ref()), "{raw:?}");
            }
        }
    }

    #[test]
    fn should_recognize_canonical_structure() {
        for text in [
            "",
            "Title",
            "Title...",
            "Title..",
            "𝐄𝐢𝐧 Möbel 🌿✨Anrichte",
            "Vase  rot",
        ] {
            assert!(Title::has_canonical_structure(text), "{text:?}");
        }
        for text in [
            "title".to_owned(),
            " Title".to_owned(),
            "Title ".to_owned(),
            "Title.".to_owned(),
            "𝐄𝐢𝐧 Möbel.".to_owned(),
            "A".repeat(129),
        ] {
            assert!(!Title::has_canonical_structure(&text), "{text:?}");
        }
    }
}
