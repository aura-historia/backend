//! Whether two product URLs name one page in two language versions
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`).

use super::is_identity_token_of;
use url::Url;

/// Directory names that name a language in words, in English or in the language itself without
/// diacritics, as a shop names the directory of a language version (`/english/…`). A language
/// code (`/en/…`) before the path is a spelling ([`product_spelling`](super::product_spelling)); a two- or three-letter
/// directory elsewhere names a category as often as a language, so codes are no language
/// directory.
const LANGUAGE_DIRECTORIES: &[&str] = &[
    "english",
    "japanese",
    "nihongo",
    "german",
    "deutsch",
    "french",
    "francais",
    "spanish",
    "espanol",
    "italian",
    "italiano",
    "dutch",
    "nederlands",
    "portuguese",
    "portugues",
    "chinese",
    "korean",
    "russian",
    "swedish",
    "svenska",
    "danish",
    "dansk",
    "norwegian",
    "norsk",
    "finnish",
    "suomi",
    "polish",
    "polski",
    "czech",
];

/// Whether two [product spellings](super::product_spelling) name one page in two language versions: on
/// one origin and with one query, their paths differ in one directory only, a language directory
/// ([`LANGUAGE_DIRECTORIES`]) on at least one side, and `identity` is a whole query value or path
/// segment of them ([`is_identity_token_of`]), so the URLs themselves name the product the shared
/// identity stands for.
pub(super) fn are_language_directory_versions(spelling: &Url, other: &Url, identity: &str) -> bool {
    let segments: Vec<&str> = spelling.path().split('/').collect();
    let other_segments: Vec<&str> = other.path().split('/').collect();
    let differing: Vec<usize> = (0..segments.len())
        .filter(|&index| other_segments.get(index) != Some(&segments[index]))
        .collect();
    let differs_in_one_language_directory = match differing[..] {
        [index] => {
            segments.len() == other_segments.len()
                && index + 1 < segments.len()
                && (is_language_directory(segments[index])
                    || is_language_directory(other_segments[index]))
        }
        _ => false,
    };
    let shared_link = match spelling.query() {
        Some(query) => format!("{}?{query}", other.path()),
        None => other.path().to_owned(),
    };
    spelling.origin() == other.origin()
        && spelling.query() == other.query()
        && differs_in_one_language_directory
        && is_identity_token_of(identity, &shared_link)
}

fn is_language_directory(segment: &str) -> bool {
    LANGUAGE_DIRECTORIES
        .iter()
        .any(|language| segment.eq_ignore_ascii_case(language))
}
