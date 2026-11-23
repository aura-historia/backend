//! What a title rule captures besides the product's name: a segment every review page's title
//! starts or ends with, such as the shop's name, a constant SEO tail or a section label, and the
//! ListingSource's name anywhere in a title. A document `<title>`, an `og:title` or an image
//! `alt` usually carries them; the product heading does not.

use regex::regex;

/// The segments of `title` between the separators shops put between a product's name and
/// what they add to it: `|` and `｜` anywhere, a dash (`-`, `–`, `—`) with whitespace on both
/// sides, and `:`, `;` or `,` before whitespace, so hyphenated words, date ranges such as
/// `1860–1949` and times keep together. Empty segments are left out.
fn segments(title: &str) -> Vec<&str> {
    regex!(r"\s*[|｜]\s*|\s+[-–—]\s+|[:;,]\s+")
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

/// Whether `title` holds the ListingSource `name` as whole words, ignoring case and how
/// whitespace runs: `Holztisch | VIADUKT*3` holds `Viadukt*3`, while `Antiquariat` does not
/// hold `Antiquar`. A blank name is held by no title.
pub(super) fn holds_listing_source_name(title: &str, name: &str) -> bool {
    let name = comparison_key(name);
    if name.is_empty() {
        return false;
    }
    let title = comparison_key(title);
    let is_word_char = |character: char| character.is_alphanumeric();
    title.match_indices(&name).any(|(start, _)| {
        let end = start + name.len();
        let joins_before = title[..start].chars().next_back().is_some_and(is_word_char)
            && name.starts_with(is_word_char);
        let joins_after =
            title[end..].chars().next().is_some_and(is_word_char) && name.ends_with(is_word_char);
        !joins_before && !joins_after
    })
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
                shared(&["Shop Navarro: Oak table", "SHOP  navarro: Walnut chair"]),
                vec![values(&["Shop Navarro"]), values(&["SHOP  navarro"])]
            );
        }

        /// LIGHTO's `og:title` ends with a full-width bar and no space, Galerie Maurer's with
        /// a spaced em dash after a range that keeps a spaced hyphen.
        #[rstest::rstest]
        #[case(&["花瓶｜LIGHTO", "茶碗｜LIGHTO"], "LIGHTO")]
        #[case(&["Landschaften 1970 - 90 — Galerie Maurer", "Akt — Galerie Maurer"], "Galerie Maurer")]
        #[case(&["La voz interior - Libro de Jordi Sierra i Fabra | SM", "El lector - Libro de Ana Alcolea | SM"], "SM")]
        #[case(&["Zitrone – Emmanuel Walderdorff Galerie", "Apfel – Emmanuel Walderdorff Galerie"], "Emmanuel Walderdorff Galerie")]
        fn should_split_at_each_kind_of_separator(#[case] titles: &[&str], #[case] tail: &str) {
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

        #[rstest::rstest]
        #[case::hyphenated_words(&["Leder-Sofa", "Leder-Sessel"])]
        #[case::date_ranges(&["James Ensor (1860–1949)", "Paul Klee (1879–1940)"])]
        #[case::shared_year(&["Chair - 1900", "Table - 1900"])]
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
            "… – ABC Antiquariat Marco Pinkus & Büchersuchdienst",
            "ABC Antiquariat Marco Pinkus & Büchersuchdienst"
        )]
        #[case(
            "Mappe; Roth, Benno; Offered by Antiquariat Immanuel",
            "Antiquariat Immanuel"
        )]
        fn should_find_the_name_as_whole_words(#[case] title: &str, #[case] name: &str) {
            assert!(holds_listing_source_name(title, name), "{title}");
        }

        #[rstest::rstest]
        #[case("Antiquariatsbestand", "Antiquariat")]
        #[case("Oak table", "Navarro")]
        #[case("Oak table", " ")]
        fn should_not_find_a_name_inside_another_word_or_a_blank_name(
            #[case] title: &str,
            #[case] name: &str,
        ) {
            assert!(!holds_listing_source_name(title, name), "{title}");
        }
    }
}
