//! The text a console list searches its rows for.

/// The longest search text; a longer one cannot name a shop, a host or an id.
pub const MAX_SEARCH_TERM_CHARS: usize = 100;

/// Non-blank search text without control characters, at most [`MAX_SEARCH_TERM_CHARS`] long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerm(String);

impl SearchTerm {
    /// The trimmed `raw` as a search term, or `None` when it is blank, longer than
    /// [`MAX_SEARCH_TERM_CHARS`] or holds a control character.
    pub fn new(raw: &str) -> Option<Self> {
        let term = raw.trim();
        (!term.is_empty()
            && term.chars().count() <= MAX_SEARCH_TERM_CHARS
            && !term.chars().any(char::is_control))
        .then(|| Self(term.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// An `ILIKE` pattern, with the default `\` escape, that matches text containing the term
    /// literally: the term's own `%`, `_` and `\` match themselves.
    pub fn contains_pattern(&self) -> String {
        let mut pattern = String::with_capacity(self.0.len() + 2);
        pattern.push('%');
        for character in self.0.chars() {
            if matches!(character, '%' | '_' | '\\') {
                pattern.push('\\');
            }
            pattern.push(character);
        }
        pattern.push('%');
        pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_trim_the_term_and_refuse_blank_long_or_control_text() {
        assert_eq!(
            SearchTerm::new("  Antik \t")
                .as_ref()
                .map(SearchTerm::as_str),
            Some("Antik")
        );
        assert_eq!(SearchTerm::new("   "), None);
        assert_eq!(SearchTerm::new(""), None);
        assert_eq!(SearchTerm::new("a\nb"), None);
        assert!(SearchTerm::new(&"a".repeat(MAX_SEARCH_TERM_CHARS)).is_some());
        assert_eq!(
            SearchTerm::new(&"a".repeat(MAX_SEARCH_TERM_CHARS + 1)),
            None
        );
    }

    #[test]
    fn should_match_the_wildcards_of_the_term_literally() {
        let term = SearchTerm::new("50%_off\\").unwrap_or_else(|| panic!("a search term"));

        assert_eq!(term.contains_pattern(), "%50\\%\\_off\\\\%");
    }
}
