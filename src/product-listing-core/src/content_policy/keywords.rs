/// Lowercase keywords that indicate one sensitive content category.
pub(super) struct KeywordSet {
    /// Whole words or phrases, bounded by non-alphanumeric characters or the text edges.
    pub(super) words: &'static [&'static str],
}

impl KeywordSet {
    /// Whether `text`, already lowercase, contains any keyword of this set.
    pub(super) fn matches(&self, text: &str) -> bool {
        self.words.iter().any(|word| contains_word(text, word))
    }
}

/// Returns `true` when `text` (expected to already be lowercase) contains
/// `keyword` surrounded by non-alphanumeric boundaries (or string edges).
///
/// This prevents partial-word false positives, e.g. "nsdap" matching inside
/// "nsdapper".  Multi-word keywords naturally include spaces which are not
/// alphanumeric and therefore act as their own boundaries.
fn contains_word(text: &str, keyword: &str) -> bool {
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
