use std::borrow::Cow;

/// Lowercase keywords that indicate one sensitive content category.
///
/// Word boundaries are Unicode-aware: an alphanumeric character continues a word, except in
/// scripts written without spaces between words (Chinese, Japanese, Korean, Thai), where a
/// Latin keyword often sits directly against the text. Multi-word keywords include spaces or
/// hyphens, which act as their own boundaries.
pub(super) struct KeywordSet {
    /// Whole words or phrases, bounded by non-alphanumeric characters or the text edges, so
    /// "nsdap" does not match inside "nsdapper".
    pub(super) words: &'static [&'static str],
    /// Word beginnings, bounded on the left only, so one stem covers inflections and compounds
    /// ("pornogr" covers "pornography", "pornographisch" and "pornográfico").
    pub(super) word_prefixes: &'static [&'static str],
    /// Terms of scripts written without spaces between words (Chinese, Japanese), which match
    /// anywhere in the text.
    pub(super) substrings: &'static [&'static str],
    /// Phrases that contain a keyword in an unrelated sense ("naked eye", "长春宫"). They are
    /// blanked out, wherever they occur, before the keywords are matched.
    pub(super) exclusions: &'static [&'static str],
}

impl KeywordSet {
    /// Whether `text`, already lowercase, contains any keyword of this set.
    pub(super) fn matches(&self, text: &str) -> bool {
        let text = &*self.without_exclusions(text);
        self.words.iter().any(|word| contains_word(text, word))
            || self
                .word_prefixes
                .iter()
                .any(|prefix| contains_word_start(text, prefix))
            || self
                .substrings
                .iter()
                .any(|substring| text.contains(substring))
    }

    fn without_exclusions<'t>(&self, text: &'t str) -> Cow<'t, str> {
        self.exclusions
            .iter()
            .fold(Cow::Borrowed(text), |text, exclusion| {
                if text.contains(exclusion) {
                    Cow::Owned(text.replace(exclusion, " "))
                } else {
                    text
                }
            })
    }

    #[cfg(test)]
    pub(super) fn keywords(&self) -> impl Iterator<Item = &'static str> {
        self.words
            .iter()
            .chain(self.word_prefixes)
            .chain(self.substrings)
            .chain(self.exclusions)
            .copied()
    }
}

fn contains_word(text: &str, word: &str) -> bool {
    word_starts(text, word).any(|start| ends_word(text, start + word.len()))
}

fn contains_word_start(text: &str, prefix: &str) -> bool {
    word_starts(text, prefix).next().is_some()
}

/// Byte offsets of every occurrence of `keyword` in `text` that begins a word, overlapping
/// occurrences included.
fn word_starts<'a>(text: &'a str, keyword: &'a str) -> impl Iterator<Item = usize> + 'a {
    let first_char_len = keyword.chars().next().map_or(1, char::len_utf8);
    let mut from = 0;
    std::iter::from_fn(move || {
        while let Some(offset) = text.get(from..)?.find(keyword) {
            let start = from + offset;
            from = start + first_char_len;
            if starts_word(text, start) {
                return Some(start);
            }
        }
        None
    })
}

fn starts_word(text: &str, start: usize) -> bool {
    !text[..start]
        .chars()
        .next_back()
        .is_some_and(continues_word)
}

fn ends_word(text: &str, end: usize) -> bool {
    !text[end..].chars().next().is_some_and(continues_word)
}

fn continues_word(character: char) -> bool {
    character.is_alphanumeric() && !is_written_without_spaces(character)
}

fn is_written_without_spaces(character: char) -> bool {
    matches!(
        character,
        '\u{0E00}'..='\u{0EFF}' // Thai, Lao
            | '\u{1100}'..='\u{11FF}' // Hangul Jamo
            | '\u{3040}'..='\u{30FF}' // Hiragana, Katakana
            | '\u{3130}'..='\u{318F}' // Hangul compatibility Jamo
            | '\u{31F0}'..='\u{31FF}' // Katakana phonetic extensions
            | '\u{3400}'..='\u{4DBF}' // CJK extension A
            | '\u{4E00}'..='\u{9FFF}' // CJK unified ideographs
            | '\u{AC00}'..='\u{D7AF}' // Hangul syllables
            | '\u{F900}'..='\u{FAFF}' // CJK compatibility ideographs
            | '\u{FF66}'..='\u{FF9F}' // Halfwidth Katakana
            | '\u{20000}'..='\u{3134F}' // CJK extensions B to G
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: KeywordSet = KeywordSet {
        words: &["nude", "naked", "ню", "femme nue"],
        word_prefixes: &["eroti"],
        substrings: &["裸体"],
        exclusions: &["naked eye"],
    };

    #[rstest::rstest]
    #[case("nude", true)]
    #[case("female nude, oil", true)]
    #[case("semi-nude figure", true)]
    #[case("(nude)", true)]
    #[case("nudel", false)]
    #[case("denuded", false)]
    #[case("в стиле ню", true)]
    #[case("меню ресторана", false)]
    #[case("une femme nue", true)]
    #[case("femme nuée", false)]
    #[case("erotic", true)]
    #[case("erotische postkarte", true)]
    #[case("heroticon", false)]
    #[case("清代裸体画", true)]
    #[case("", false)]
    #[case("日本昭和nude写真集", true)]
    #[case("nudeの写真", true)]
    #[case("not visible to the naked eye", false)]
    #[case("naked eye, naked figure", true)]
    fn should_match_keywords_by_kind(#[case] text: &str, #[case] expected: bool) {
        assert_eq!(SET.matches(text), expected);
    }

    #[test]
    fn should_find_a_word_after_an_occurrence_inside_another_word() {
        assert!(contains_word("nudenude nude", "nude"));
        assert!(contains_word("ss ss-rune", "ss-rune"));
    }

    #[test]
    fn should_continue_past_a_multibyte_first_character() {
        assert!(contains_word("ünü nü", "nü"));
        assert!(contains_word("éérotique érotique", "érotique"));
        assert!(!contains_word("éérotique", "érotique"));
    }
}
