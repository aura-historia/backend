//! Reviewed per-source description boilerplate.
//!
//! Shops repeat sentences about shipping, invoices or payment across every listing. A reviewed
//! set of such sentences for one listing source is stripped from that source's descriptions
//! before language detection, so the canonical description describes the item. Repetition alone
//! never selects a sentence: only sentences a reviewer approved are stripped, because a repeated
//! sentence can be an item fact (whether a postcard was posted). See
//! `docs/product-listing.md#description-boilerplate`.
//!
//! The segmentation and the sentence key are shared by stripping and by the offline analysis that
//! proposes candidates, so an approved candidate matches exactly the sentences it was counted
//! from.

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Longest sentence key, in bytes, that a reviewed set may hold. Longer repeated text is a
/// template paragraph, not a sentence, and is left to the description.
pub const MAX_BOILERPLATE_SENTENCE_BYTES: usize = 1_000;

/// Normalized identity of one description sentence: whitespace runs collapse to one space and
/// letters fold to lower case, so layout and capitalization differences still match.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoilerplateSentence(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoilerplateSentenceError {
    #[error("boilerplate sentence is blank")]
    Blank,
    #[error("boilerplate sentence exceeds {MAX_BOILERPLATE_SENTENCE_BYTES} bytes")]
    TooLong,
}

impl BoilerplateSentence {
    pub fn new(sentence: &str) -> Result<Self, BoilerplateSentenceError> {
        let key = sentence_key(sentence);
        if key.is_empty() {
            return Err(BoilerplateSentenceError::Blank);
        }
        if key.len() > MAX_BOILERPLATE_SENTENCE_BYTES {
            return Err(BoilerplateSentenceError::TooLong);
        }
        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn sentence_key(sentence: &str) -> String {
    sentence
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The reviewed sentences one listing source strips from its descriptions. An empty set strips
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DescriptionBoilerplate(BTreeSet<BoilerplateSentence>);

impl FromIterator<BoilerplateSentence> for DescriptionBoilerplate {
    fn from_iter<I: IntoIterator<Item = BoilerplateSentence>>(sentences: I) -> Self {
        Self(sentences.into_iter().collect())
    }
}

impl DescriptionBoilerplate {
    pub const fn new() -> Self {
        Self(BTreeSet::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn sentences(&self) -> impl Iterator<Item = &BoilerplateSentence> {
        self.0.iter()
    }

    pub fn contains(&self, sentence: &str) -> bool {
        self.0
            .contains(&BoilerplateSentence(sentence_key(sentence)))
    }

    /// Order-independent SHA-256 of the set, recorded as normalization provenance. `None` for an
    /// empty set, which strips nothing.
    pub fn sha256(&self) -> Option<[u8; 32]> {
        if self.0.is_empty() {
            return None;
        }
        let mut digest = Sha256::new();
        for sentence in &self.0 {
            digest.update(sentence.as_str().as_bytes());
            digest.update([0]);
        }
        Some(digest.finalize().into())
    }

    /// Removes every reviewed sentence from the description fragments. Fails safe: when nothing
    /// but boilerplate would remain, the fragments are returned unchanged, so stripping never
    /// clears a description.
    pub fn strip(&self, fragments: Vec<String>) -> Vec<String> {
        if self.0.is_empty() {
            return fragments;
        }
        let stripped = fragments
            .iter()
            .map(|fragment| self.strip_fragment(fragment))
            .collect::<Vec<_>>();
        if stripped.iter().all(|fragment| fragment.trim().is_empty()) {
            return fragments;
        }
        stripped
    }

    fn strip_fragment(&self, fragment: &str) -> String {
        let mut kept: Vec<(&str, &str)> = Vec::new();
        for segment in segments(fragment) {
            if !self.contains(segment.sentence) {
                kept.push((segment.sentence, segment.separator));
                continue;
            }
            // Keep the stronger separator around a removed sentence, so a removed sentence that
            // ended a line or paragraph does not join its neighbours into one line.
            if let Some((_, previous)) = kept.last_mut()
                && line_breaks(segment.separator) > line_breaks(previous)
            {
                *previous = segment.separator;
            }
        }
        let mut text = String::with_capacity(fragment.len());
        for (sentence, separator) in kept {
            text.push_str(sentence);
            text.push_str(separator);
        }
        text.trim().to_owned()
    }
}

fn line_breaks(separator: &str) -> usize {
    separator.matches('\n').count()
}

/// Splits description text into its sentences, trimmed and in order. Analysis counts these
/// sentences; stripping removes the reviewed ones.
pub fn description_sentences(text: &str) -> impl Iterator<Item = &str> {
    segments(text).map(|segment| segment.sentence)
}

/// One sentence and the whitespace that follows it up to the next sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment<'a> {
    sentence: &'a str,
    separator: &'a str,
}

/// Sentence boundaries: a line break; `.`, `!`, `?` or `…` followed by whitespace and text that
/// does not start in lower case; or a full-width `。`, `！`, `？`, which need neither. Closing
/// quotes and brackets stay with their sentence. The lower-case rule keeps abbreviations such as
/// `MwSt. liegt bei` together; others such as `ca. 1900` still split. Analysis and stripping see
/// the same segments, so a split never matches a sentence it was not counted from.
fn segments(text: &str) -> impl Iterator<Item = Segment<'_>> {
    let mut rest = text.trim_start();
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = sentence_end(rest);
        let (sentence, tail) = rest.split_at(end);
        let separator_len = tail.len() - tail.trim_start().len();
        let (separator, next) = tail.split_at(separator_len);
        rest = next;
        Some(Segment {
            sentence: sentence.trim_end(),
            separator,
        })
    })
}

fn sentence_end(text: &str) -> usize {
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\n' || character == '\r' {
            return index;
        }
        let full_width = matches!(character, '。' | '！' | '？');
        if !full_width && !matches!(character, '.' | '!' | '?' | '…') {
            continue;
        }
        let mut end = index + character.len_utf8();
        while let Some(&(closer_index, closer)) = characters.peek() {
            if !is_closing_mark(closer) {
                break;
            }
            end = closer_index + closer.len_utf8();
            characters.next();
        }
        match characters.peek() {
            None => return end,
            Some(_) if full_width => return end,
            Some(&(_, next)) if next.is_whitespace() && !continues_in_lower_case(&text[end..]) => {
                return end;
            }
            Some(_) => {}
        }
    }
    text.len()
}

/// Whether the text after a terminator continues the sentence on the same line in lower case.
fn continues_in_lower_case(rest: &str) -> bool {
    let next = rest.trim_start_matches(|character: char| {
        character.is_whitespace() && character != '\n' && character != '\r'
    });
    next.chars().next().is_some_and(char::is_lowercase)
}

fn is_closing_mark(character: char) -> bool {
    matches!(
        character,
        '"' | '\'' | ')' | ']' | '”' | '’' | '»' | '」' | '』' | '）'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn boilerplate(sentences: &[&str]) -> DescriptionBoilerplate {
        sentences
            .iter()
            .map(|sentence| BoilerplateSentence::new(sentence).unwrap())
            .collect()
    }

    fn strip(sentences: &[&str], fragments: &[&str]) -> Vec<String> {
        boilerplate(sentences).strip(fragments.iter().map(|&text| text.to_owned()).collect())
    }

    #[rstest]
    #[case("Eine Vase. Gut erhalten!", &["Eine Vase.", "Gut erhalten!"])]
    #[case("Line one\nLine two", &["Line one", "Line two"])]
    #[case("Sie sagte: \"Ja.\" Dann ging sie.", &["Sie sagte: \"Ja.\"", "Dann ging sie."])]
    #[case("Version 1.5 bleibt ganz", &["Version 1.5 bleibt ganz"])]
    #[case("Mit MwSt. ausgewiesen. Danke.", &["Mit MwSt. ausgewiesen.", "Danke."])]
    #[case("Um ca. 1900 gefertigt.", &["Um ca.", "1900 gefertigt."])]
    #[case("送料無料です。即日発送！", &["送料無料です。", "即日発送！"])]
    #[case("  \n ", &[])]
    fn should_split_description_into_sentences(#[case] text: &str, #[case] expected: &[&str]) {
        assert_eq!(expected, description_sentences(text).collect::<Vec<_>>());
    }

    #[test]
    fn should_match_sentences_regardless_of_whitespace_and_case() {
        assert_eq!(
            BoilerplateSentence::new("Free  shipping\thas NO tracking number.").unwrap(),
            BoilerplateSentence::new("free shipping has no tracking number.").unwrap()
        );
    }

    #[rstest]
    #[case(" \n ", BoilerplateSentenceError::Blank)]
    #[case(&"a".repeat(MAX_BOILERPLATE_SENTENCE_BYTES + 1), BoilerplateSentenceError::TooLong)]
    fn should_reject_unusable_sentence(
        #[case] sentence: &str,
        #[case] expected: BoilerplateSentenceError,
    ) {
        assert_eq!(Err(expected), BoilerplateSentence::new(sentence));
    }

    #[test]
    fn should_strip_reviewed_sentences_and_keep_item_text() {
        let stripped = strip(
            &[
                "El envío gratuito no tiene número de seguimiento.",
                "Pago por transferencia.",
            ],
            &[
                "Libro antiguo, encuadernación en piel. El envío gratuito no tiene número de seguimiento. Buen estado.",
                "Pago por transferencia.",
            ],
        );
        assert_eq!(
            vec![
                "Libro antiguo, encuadernación en piel. Buen estado.".to_owned(),
                String::new()
            ],
            stripped
        );
    }

    #[test]
    fn should_keep_line_break_of_a_removed_sentence() {
        let stripped = strip(
            &["Eine Rechnung mit MwSt. liegt bei."],
            &["Biedermeier Kommode. Eine Rechnung mit MwSt. liegt bei.\nKirschbaum furniert."],
        );
        assert_eq!(
            vec!["Biedermeier Kommode.\nKirschbaum furniert.".to_owned()],
            stripped
        );
    }

    #[test]
    fn should_keep_postcard_state_when_only_shop_notice_is_reviewed() {
        let notice = "Versand innerhalb Deutschlands als Großbrief.";
        for state in ["Gelaufen.", "Nicht gelaufen mit der Post."] {
            let stripped = strip(
                &[notice],
                &[&format!("Ansichtskarte Berlin. {state} {notice}")],
            );
            assert_eq!(vec![format!("Ansichtskarte Berlin. {state}")], stripped);
        }
    }

    #[test]
    fn should_keep_original_description_when_only_boilerplate_remains() {
        let fragments = ["Save on postage: combine your purchases.", "  "];
        assert_eq!(
            fragments.map(str::to_owned).to_vec(),
            strip(&["Save on postage: combine your purchases."], &fragments)
        );
    }

    #[test]
    fn should_strip_nothing_without_reviewed_sentences() {
        let fragments = vec!["Free shipping.".to_owned()];
        assert_eq!(
            fragments.clone(),
            DescriptionBoilerplate::new().strip(fragments)
        );
    }

    #[test]
    fn should_fingerprint_set_independently_of_order() {
        let first = boilerplate(&["A.", "B."]);
        assert_eq!(first.sha256(), boilerplate(&["B.", "a."]).sha256());
        assert_ne!(first.sha256(), boilerplate(&["A."]).sha256());
        assert_eq!(None, DescriptionBoilerplate::new().sha256());
    }
}
