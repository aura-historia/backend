//! What the live schemas' titles carry beside the product's name that no blocking check can tell
//! for certain, so the audit reports it for the review agents to judge: the product's own article,
//! stock or item number, and words every captured title starts with, such as a section heading
//! read with the name. A shop may name its products so, as a dealer of one kind of item does, so
//! these open a review but never refuse an approval.

use crate::review::model::SchemaMatrix;
use crate::scraper::plausibility::is_link_or_path_identity;

/// The shortest identity code a title is checked for: shorter ones, such as a model's `14`, are
/// too often part of the name.
const MIN_IDENTITY_CODE_CHARS: usize = 4;

/// The fewest leading words every title must share to count as a heading read with the name;
/// two shared words, such as "Pair of" or "Antique French", are common in one shop's names.
const MIN_SHARED_LEADING_WORDS: usize = 3;

/// A title that holds the product's own identity code.
pub(super) const HOLDS_IDENTITY: &str = "title HOLDS_IDENTITY";
/// Titles that all start with the same words.
pub(super) const SHARED_LEADING_WORDS: &str = "title SHARED_LEADING_WORDS";

/// The title hints of the candidates of `matrix`, each code once, from the extractions runtime
/// selection could use (`apply_ok`).
pub(super) fn title_hints(matrix: &SchemaMatrix) -> Vec<&'static str> {
    let mut hints = Vec::new();
    for candidate in &matrix.candidates {
        let extractions: Vec<(&str, &str)> = candidate
            .pages
            .iter()
            .filter(|page| page.apply_ok)
            .filter_map(|page| page.extracted.as_ref())
            .filter_map(|raw| Some((raw.title()?.trim(), raw.source_listing_id.trim())))
            .collect();
        if extractions
            .iter()
            .any(|(title, identity)| holds_identity(title, identity))
        {
            hints.push(HOLDS_IDENTITY);
        }
        let titles: Vec<&str> = extractions.iter().map(|(title, _)| *title).collect();
        if shares_leading_words(&titles) {
            hints.push(SHARED_LEADING_WORDS);
        }
    }
    hints.sort_unstable();
    hints.dedup();
    hints
}

/// Whether `title` holds a code of `identity` as a whole token, ignoring case: the identity
/// itself, or one of its parts between spaces, labels and separators that has a digit, such as
/// `130951` of `Item#: 130951` or `2118031` of `SKU-2118031`, of at least
/// [`MIN_IDENTITY_CODE_CHARS`] characters. A year such as `1775` is no code, since a dating in the
/// name repeats it, and neither is any part of an identity that is a URL or path.
fn holds_identity(title: &str, identity: &str) -> bool {
    if is_link_or_path_identity(identity) {
        return false;
    }
    let title = title.to_lowercase();
    let identity = identity.to_lowercase();
    std::iter::once(identity.as_str())
        .chain(identity.split(|character: char| {
            character.is_whitespace() || matches!(character, ':' | '#' | ',' | ';' | '-' | '/')
        }))
        .map(|code| code.trim_matches(|character: char| !character.is_alphanumeric()))
        .filter(|code| {
            code.chars().count() >= MIN_IDENTITY_CODE_CHARS
                && code.chars().any(|character| character.is_ascii_digit())
                && !is_year(code)
        })
        .any(|code| holds_token(&title, code))
}

/// Whether `code` reads as a year a dating names, from 1000 to 2099.
fn is_year(code: &str) -> bool {
    code.len() == 4
        && code
            .parse::<u16>()
            .is_ok_and(|year| (1000..=2099).contains(&year))
}

/// Whether `text` holds `token` with no letter or digit directly before or after it.
fn holds_token(text: &str, token: &str) -> bool {
    let is_word_char = |character: char| character.is_alphanumeric();
    text.match_indices(token).any(|(start, _)| {
        let end = start + token.len();
        !text[..start].chars().next_back().is_some_and(is_word_char)
            && !text[end..].chars().next().is_some_and(is_word_char)
    })
}

/// Whether at least two `titles` all start with the same [`MIN_SHARED_LEADING_WORDS`] or more
/// words, ignoring case, and each goes on beyond them.
fn shares_leading_words(titles: &[&str]) -> bool {
    if titles.len() < 2 {
        return false;
    }
    let words: Vec<Vec<String>> = titles
        .iter()
        .map(|title| title.split_whitespace().map(str::to_lowercase).collect())
        .collect();
    let shortest = words.iter().map(Vec::len).min().unwrap_or(0);
    let shared = (0..shortest)
        .take_while(|&index| words.iter().all(|title| title[index] == words[0][index]))
        .count();
    shared >= MIN_SHARED_LEADING_WORDS && shared < shortest
}

#[cfg(test)]
mod tests {
    use super::*;

    mod identity {
        use super::*;

        /// Burbri Antiek's heading adds the article number; Antiqueshop Handle's adds its item
        /// number in brackets.
        #[rstest::rstest]
        #[case("Antiek hek 2118031", "2118031")]
        #[case("Lace collar (商品番号 cm-396)", "cm-396")]
        #[case("Oak chair - Ref. A1234", "Ref: A1234")]
        #[case("Antiek hek 2118031", "SKU-2118031")]
        fn should_find_the_identity_in_the_title(#[case] title: &str, #[case] identity: &str) {
            assert!(holds_identity(title, identity), "{title}");
        }

        #[rstest::rstest]
        #[case::labelled_identity_not_in_the_title("Microcosm of London", "Item#: 130951")]
        #[case::short_model_number("Thonet bentwood chair No. 14", "14")]
        #[case::code_inside_a_longer_number("Antiek hek 21180312", "2118031")]
        #[case::identity_without_a_digit("Walnut chair", "walnut")]
        #[case::blank_identity("Walnut chair 2023", "")]
        #[case::year_in_a_dating("George III mahogany cabinet, circa 1775", "Lot 1775")]
        #[case::url_identity("Oak chair 1234", "https://shop.example/p#1234")]
        fn should_not_find_what_is_no_identity_code(#[case] title: &str, #[case] identity: &str) {
            assert!(!holds_identity(title, identity), "{title}");
        }
    }

    mod leading_words {
        use super::*;

        /// Atlas en Kaart's heading puts a section label before each map's name.
        #[test]
        fn should_find_words_every_title_starts_with() {
            assert!(shares_leading_words(&[
                "Informatie over de Kuyperkaart Gemeente Bathmen",
                "Informatie over de Kuyperkaart Gemeente Diepenheim",
                "Informatie over de Kuyperkaart Gemeente Markelo",
            ]));
        }

        #[rstest::rstest]
        #[case::two_shared_words(&["Pair of chairs", "Pair of George III tables"])]
        #[case::different_starts(&["Informatie over Bathmen", "Kaart van Markelo", "Atlas"])]
        #[case::title_that_is_only_the_shared_words(&["Antique French Stone", "Antique french stone fireplace"])]
        #[case::single_title(&["Informatie over de Kuyperkaart Gemeente Bathmen"])]
        fn should_not_find_what_is_no_shared_heading(#[case] titles: &[&str]) {
            assert!(!shares_leading_words(titles), "{titles:?}");
        }
    }
}
