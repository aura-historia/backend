/// Lowercase keywords that indicate one sensitive content category.
///
/// Word boundaries are Unicode-aware: any alphanumeric character, in any script, continues a
/// word. Multi-word keywords include spaces or hyphens, which act as their own boundaries.
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
}

impl KeywordSet {
    /// Whether `text`, already lowercase, contains any keyword of this set.
    pub(super) fn matches(&self, text: &str) -> bool {
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

    #[cfg(test)]
    pub(super) fn keywords(&self) -> impl Iterator<Item = &'static str> {
        self.words
            .iter()
            .chain(self.word_prefixes)
            .chain(self.substrings)
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
        .is_some_and(char::is_alphanumeric)
}

fn ends_word(text: &str, end: usize) -> bool {
    !text[end..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: KeywordSet = KeywordSet {
        words: &["nude", "ню", "femme nue"],
        word_prefixes: &["eroti"],
        substrings: &["裸体"],
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
