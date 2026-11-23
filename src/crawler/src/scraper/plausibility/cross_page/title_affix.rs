//! What a title rule captures besides the product's name: a segment after a bar that every review
//! page's title starts or ends with, such as the shop's name or a constant SEO tail, and the
//! ListingSource's name at the start or the end of a title. A document `<title>`, an `og:title` or
//! an image `alt` usually carries them; the product heading does not.
//!
//! Both checks block approval, so they stay narrow enough not to refuse a correct product
//! heading. Commas, colons and dashes are no separators here: a heading joins the product's own
//! facts with them, as in "Chair, 19th century", "Tea bowl - Edo period" or "Beloved: A Novel",
//! which a specialist dealer's review pages may all share. A shop name after such a separator is
//! still found by the name check, and a tail that differs per page is left to the reviewer.

use regex::regex;

/// The segments of `title` between the bars (`|`, `｜`) that shops put between a product's name
/// and what they add to it. Empty segments are left out.
fn segments(title: &str) -> Vec<&str> {
    regex!(r"\s*[|｜]\s*")
        .split(title)
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// A title as compared with others: lowercased and with each whitespace run one space.
fn comparison_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// For each of `titles`, the leading and trailing segments every other title starts or ends with
/// too, as this title spells them. A segment counts only when at least two titles share it, each
/// with more than that segment, and it has a letter, so a shared year or number is no affix.
/// Every title must share it: titles of different products may share a word, but not the shop's
/// name or a constant tail on every review page. Blank titles, and titles of separators alone, are
/// left out, as a page that shows no name says nothing about the others.
pub(super) fn shared_segments(titles: &[Option<&str>]) -> Vec<Vec<String>> {
    let split: Vec<Option<Vec<&str>>> = titles
        .iter()
        .map(|title| {
            title
                .map(str::trim)
                .map(segments)
                .filter(|segments| !segments.is_empty())
        })
        .collect();
    let titled: Vec<&Vec<&str>> = split.iter().flatten().collect();
    let shared = |pick: SegmentPick| -> Option<String> {
        let mut keys = titled.iter().map(|segments| {
            (segments.len() > 1).then(|| comparison_key(pick(segments.as_slice())))
        });
        let first = keys.next()??;
        (titled.len() > 1
            && first.chars().any(char::is_alphabetic)
            && keys.all(|key| key.as_ref() == Some(&first)))
        .then_some(first)
    };
    let leading = shared(first_segment);
    let trailing = shared(last_segment);
    split
        .iter()
        .map(|segments| {
            let Some(segments) = segments else {
                return Vec::new();
            };
            [
                leading.as_ref().map(|_| first_segment(segments)),
                trailing.as_ref().map(|_| last_segment(segments)),
            ]
            .into_iter()
            .flatten()
            .map(str::to_owned)
            .collect()
        })
        .collect()
}

/// Picks the leading or the trailing one of a title's segments, of which it has at least one.
type SegmentPick = for<'t, 's> fn(&'s [&'t str]) -> &'t str;

fn first_segment<'t>(segments: &[&'t str]) -> &'t str {
    segments[0]
}

fn last_segment<'t>(segments: &[&'t str]) -> &'t str {
    segments[segments.len() - 1]
}

/// Whether `title` ends with the ListingSource `name` as whole words, or starts with it before a
/// separator (`|`, `｜`, a dash, `:`, `;` or `,`), ignoring case, how whitespace runs and the
/// punctuation around them, besides naming something else: `Holztisch | VIADUKT*3` and
/// `Zitrone – Emmanuel Walderdorff Galerie.` do, while `Thonet bentwood chair` does not affix the
/// dealer `Thonet`, which names the maker, nor `Antiquariatsbestand` the shop `Antiquariat`. A
/// blank name is affixed to no title.
pub(super) fn affixes_listing_source_name(title: &str, name: &str) -> bool {
    let is_word_char = |character: char| character.is_alphanumeric();
    let name = comparison_key(name);
    let name = name.trim_matches(|character: char| !is_word_char(character));
    let title = comparison_key(title);
    let title = title.trim_matches(|character: char| !is_word_char(character));
    if name.is_empty() || title.len() <= name.len() {
        return false;
    }
    let ends_with_name = title
        .strip_suffix(name)
        .and_then(|rest| rest.chars().next_back())
        .is_some_and(|before| !is_word_char(before));
    let starts_with_name = title.strip_prefix(name).is_some_and(|rest| {
        rest.trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, '.' | ')' | '"' | '\'')
        })
        .starts_with(['|', '｜', '-', '–', '—', ':', ';', ','])
    });
    ends_with_name || starts_with_name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(titles: &[&str]) -> Vec<Vec<String>> {
        let titles: Vec<Option<&str>> = titles.iter().copied().map(Some).collect();
        shared_segments(&titles)
    }

    fn values(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    mod shared_segments {
        use super::*;

        /// VIADUKT*3's document `<title>` adds the category path and the shop's name.
        #[test]
        fn should_name_the_trailing_segment_every_title_ends_with() {
            assert_eq!(
                shared(&[
                    "Holztisch | Holztische | Tische | VIADUKT*3",
                    "Stuhl | Stühle | VIADUKT*3",
                ]),
                vec![values(&["VIADUKT*3"]), values(&["VIADUKT*3"])]
            );
        }

        #[test]
        fn should_name_a_leading_segment_spelled_differently_per_page() {
            assert_eq!(
                shared(&["Shop Navarro | Oak table", "SHOP  navarro|Walnut chair"]),
                vec![values(&["Shop Navarro"]), values(&["SHOP  navarro"])]
            );
        }

        /// LIGHTO's `og:title` ends with a full-width bar and no space; Ven y Empieza's document
        /// `<title>` adds a byline after a dash and the publisher after a bar.
        #[rstest::rstest]
        #[case(&["花瓶｜LIGHTO", "茶碗｜LIGHTO"], "LIGHTO")]
        #[case(&["La voz interior - Libro de Jordi Sierra i Fabra | SM", "El lector - Libro de Ana Alcolea | SM"], "SM")]
        fn should_split_at_each_kind_of_bar(#[case] titles: &[&str], #[case] tail: &str) {
            assert!(
                shared(titles)
                    .iter()
                    .all(|segments| segments == &values(&[tail])),
                "{titles:?}"
            );
        }

        #[test]
        fn should_name_nothing_when_one_title_ends_otherwise() {
            assert_eq!(
                shared(&["Chair | Shop", "Table | Shop", "Mirror"]),
                vec![Vec::<String>::new(); 3]
            );
        }

        /// Bogen33's image `alt` adds a category path that differs between products.
        #[test]
        fn should_name_nothing_when_the_segments_differ() {
            assert_eq!(
                shared(&[
                    "Knapp drei Meter, Leder-Sofas, Sofa",
                    "Rundum, Sessel, Stühle"
                ]),
                vec![Vec::<String>::new(); 2]
            );
        }

        /// Product headings join the product's own facts with commas, colons and dashes, and a
        /// dealer's review pages may all share one of them.
        #[rstest::rstest]
        #[case::shared_origin(&["Pair of armchairs, France", "Writing desk, France"])]
        #[case::shared_period(&["Tea bowl - Edo period", "Sake bottle - Edo period"])]
        #[case::shared_artist(&["Max Ernst: Lithographie", "Max Ernst: Radierung"])]
        #[case::shared_subtitle(&["Beloved: A Novel", "Jazz: A Novel"])]
        #[case::hyphenated_words(&["Leder-Sofa", "Leder-Sessel"])]
        #[case::shared_year(&["Chair | 1900", "Table | 1900"])]
        #[case::single_title(&["Chair | Shop"])]
        fn should_name_no_segment_that_is_no_affix(#[case] titles: &[&str]) {
            assert!(shared(titles).iter().all(Vec::is_empty), "{titles:?}");
        }

        #[test]
        fn should_leave_blank_titles_out() {
            assert_eq!(
                shared_segments(&[
                    Some("Chair | Shop"),
                    None,
                    Some(" "),
                    Some(" | "),
                    Some("Table | Shop")
                ]),
                vec![
                    values(&["Shop"]),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    values(&["Shop"])
                ]
            );
        }
    }

    mod listing_source_name {
        use super::*;

        #[rstest::rstest]
        #[case("Holztisch | Holztische | Tische | VIADUKT*3", "VIADUKT*3")]
        #[case(
            "Josef Koudelka - First Edition - Signed - Josef Koudelka - Bauman Rare Books",
            "Bauman  rare books"
        )]
        #[case(
            "Atlas – ABC Antiquariat Marco Pinkus & Büchersuchdienst",
            "ABC Antiquariat Marco Pinkus & Büchersuchdienst"
        )]
        #[case(
            "Mappe; Roth, Benno; Offered by Antiquariat Immanuel",
            "Antiquariat Immanuel"
        )]
        #[case(
            "Zitrone – Emmanuel Walderdorff Galerie.",
            "Emmanuel Walderdorff Galerie"
        )]
        #[case("Horgenglarus Stapel 1-680 Stuhl – Stuzh38", "STUZH38")]
        #[case("Shop & Co. | Oak table", "Shop & Co.")]
        fn should_find_the_name_at_the_start_or_the_end(#[case] title: &str, #[case] name: &str) {
            assert!(affixes_listing_source_name(title, name), "{title}");
        }

        /// A dealer named after the maker it sells, or a shop named with a word its products
        /// share, is named in a product heading as the maker or the word.
        #[rstest::rstest]
        #[case::maker("Thonet bentwood chair No. 14", "Thonet")]
        #[case::word_in_the_middle("Vienna Antik clock", "Antik")]
        #[case::inside_another_word("Mappe aus dem Antiquariatsbestand", "Antiquariat")]
        #[case::other_name("Oak table", "Navarro")]
        #[case::name_alone("Navarro", "Navarro")]
        #[case::blank_name("Oak table", " ")]
        fn should_not_find_a_name_that_is_no_affix(#[case] title: &str, #[case] name: &str) {
            assert!(!affixes_listing_source_name(title, name), "{title}");
        }
    }
}
