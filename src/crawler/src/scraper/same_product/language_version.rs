//! Whether two product URLs name one page in two language versions
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`): their paths differ only in
//! directories that name a language, and the product identity both pages show is part of what the
//! URLs share. A language code before the path is already a spelling
//! ([`product_spelling`](super::product_spelling)); this rule covers language directories at any
//! depth, inserted, dropped or swapped.

use super::language_codes::is_language_code;
use url::Url;

/// Languages named in words, in English and in the language itself without diacritics, as a shop
/// names the directory of a language version (`/english/…`, `/deutsch/…`). Such a directory names
/// a language version wherever it stands, also in place of an ordinary directory: a shop that
/// serves its own language under a section (`/ukiyoe/…`) and a translation under `/english/…`.
const LANGUAGE_NAMES: &[&str] = &[
    "english",
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
    "cesky",
    "cestina",
    "hungarian",
    "magyar",
    "greek",
    "turkish",
    "turkce",
    "russian",
    "russkiy",
    "chinese",
    "zhongwen",
    "japanese",
    "nihongo",
    "korean",
    "hangugeo",
    "arabic",
];

/// Whether `page` and `other`, two [page spellings](super::page_spelling), name one page in two
/// language versions: on one origin, with one query and one page name, their directories differ
/// only in language directories ([`PagePath::shared_directories`]), and `identity` is a whole
/// query value, a directory both keep or the page name, so the URLs themselves name the product
/// the shared identity stands for. A language directory is never the identity's token.
pub(super) fn are_language_versions(page: &Url, other: &Url, identity: &str) -> bool {
    if identity.is_empty() || page.origin() != other.origin() || page.query() != other.query() {
        return false;
    }
    let (Some(page_path), Some(other_path)) = (PagePath::of(page), PagePath::of(other)) else {
        return false;
    };
    if page_path.name != other_path.name {
        return false;
    }
    let Some(shared) = page_path.shared_directories(&other_path) else {
        return false;
    };
    page_path.name == identity
        || shared.contains(&identity)
        || query_values(page.query()).any(|value| value == identity)
}

/// A URL path as its directories and the name of the page they hold.
struct PagePath<'a> {
    directories: Vec<&'a str>,
    name: &'a str,
}

impl<'a> PagePath<'a> {
    fn of(url: &'a Url) -> Option<Self> {
        let mut directories: Vec<&str> = url.path_segments()?.collect();
        let name = directories.pop()?;
        Some(Self { directories, name })
    }

    /// The directories both paths keep when they differ only in language directories: when they
    /// are equal without their language directories ([`is_language_directory`]), as when one is
    /// inserted, dropped or swapped for another, or when one directory is swapped for a language
    /// named in words ([`LANGUAGE_NAMES`]). `None` for equal directories, which are one spelling,
    /// and for any other difference.
    fn shared_directories(&self, other: &Self) -> Option<Vec<&'a str>> {
        if self.directories == other.directories {
            return None;
        }
        let without_languages = |directories: &[&'a str]| -> Vec<&'a str> {
            directories
                .iter()
                .copied()
                .filter(|directory| !is_language_directory(directory))
                .collect()
        };
        let kept = without_languages(&self.directories);
        if kept == without_languages(&other.directories) {
            return Some(kept);
        }
        if self.directories.len() != other.directories.len() {
            return None;
        }
        let mut differing = (0..self.directories.len())
            .filter(|&index| self.directories[index] != other.directories[index]);
        match (differing.next(), differing.next()) {
            (Some(index), None)
                if is_language_name(self.directories[index])
                    || is_language_name(other.directories[index]) =>
            {
                let mut kept = self.directories.clone();
                kept.remove(index);
                Some(kept)
            }
            _ => None,
        }
    }
}

/// Whether a directory names a language: a language named in words ([`LANGUAGE_NAMES`]) or an
/// ISO 639-1 code ([`is_language_code`]) with an optional region or script subtag. A code names a
/// language version only where it cannot be taken for a category: inserted into or dropped from a
/// path, or swapped for another language ([`PagePath::shared_directories`]). A two-letter
/// directory in place of an ordinary one (`/de/` for `/vases/`) may be a category or a dealer's
/// initials whose products are numbered on their own.
fn is_language_directory(directory: &str) -> bool {
    is_language_name(directory) || is_language_code(directory)
}

fn is_language_name(directory: &str) -> bool {
    LANGUAGE_NAMES
        .iter()
        .any(|name| directory.eq_ignore_ascii_case(name))
}

fn query_values(query: Option<&str>) -> impl Iterator<Item = &str> {
    query
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter_map(|pair| pair.split_once('=').map(|(_, value)| value))
}

#[cfg(test)]
mod tests {
    use super::super::page_spelling;
    use super::*;

    fn language_versions(url: &str, other: &str, identity: &str) -> bool {
        are_language_versions(
            &page_spelling(&Url::parse(url).unwrap()),
            &page_spelling(&Url::parse(other).unwrap()),
            identity,
        )
    }

    /// Hara Shobo serves each print in Japanese under `/ukiyoe/` and in English under `/english/`;
    /// other shops put a language code or name at any depth, or leave their own language without
    /// one.
    #[rstest::rstest]
    #[case::directory_swapped_for_a_language_name(
        "https://hara.example/ukiyoe//ukiyoe_detail.php?print_id=24689",
        "https://hara.example/english/ukiyoe_detail.php?print_id=24689",
        "24689"
    )]
    #[case::language_name_swapped_back(
        "https://hara.example/english/ukiyoe_detail.php?print_id=24689",
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689",
        "24689"
    )]
    #[case::two_language_names(
        "https://shop.example/Japanese/prints/24689",
        "https://shop.example/english/prints/24689/",
        "24689"
    )]
    #[case::tracking_parameter(
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689&utm_source=mail",
        "https://hara.example/english/ukiyoe_detail.php?print_id=24689",
        "24689"
    )]
    #[case::two_codes_inside_the_path(
        "https://shop.example/shop/en/item.php?id=88",
        "https://shop.example/shop/de/item.php?id=88",
        "88"
    )]
    #[case::codes_with_region_subtags(
        "https://shop.example/store/pt-BR/print/88",
        "https://shop.example/store/en_gb/print/88",
        "88"
    )]
    #[case::code_inserted_inside_the_path(
        "https://shop.example/catalogue/fr/print/88",
        "https://shop.example/catalogue/print/88",
        "88"
    )]
    #[case::name_inserted_inside_the_path(
        "https://shop.example/catalogue/english/print.php?id=88",
        "https://shop.example/catalogue/print.php?id=88",
        "88"
    )]
    #[case::code_before_the_path_and_a_name_inside_it(
        "https://shop.example/en/catalogue/print.php?id=88",
        "https://shop.example/deutsch/catalogue/print.php?id=88",
        "88"
    )]
    #[case::identity_as_the_page_name(
        "https://shop.example/english/prints/hiroshige-88",
        "https://shop.example/prints/hiroshige-88",
        "hiroshige-88"
    )]
    fn should_take_two_language_versions_of_one_page(
        #[case] url: &str,
        #[case] other: &str,
        #[case] identity: &str,
    ) {
        assert!(language_versions(url, other, identity));
    }

    #[rstest::rstest]
    #[case::identity_not_in_the_urls(
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689",
        "https://hara.example/english/ukiyoe_detail.php?print_id=24689",
        "J263"
    )]
    #[case::identity_part_of_a_value(
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=124689",
        "https://hara.example/english/ukiyoe_detail.php?print_id=124689",
        "24689"
    )]
    #[case::identity_only_the_language_directory(
        "https://shop.example/catalogue/english/print.php?id=88",
        "https://shop.example/catalogue/print.php?id=88",
        "english"
    )]
    #[case::other_query(
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689&set=2",
        "https://hara.example/english/ukiyoe_detail.php?print_id=24689",
        "24689"
    )]
    #[case::no_language_directory(
        "https://shop.example/vases/item.php?id=88",
        "https://shop.example/bowls/item.php?id=88",
        "88"
    )]
    #[case::code_in_place_of_a_category(
        "https://shop.example/prints/vases/item.php?id=88",
        "https://shop.example/prints/de/item.php?id=88",
        "88"
    )]
    #[case::three_letters_no_language_code(
        "https://shop.example/catalog/art/item.php?id=88",
        "https://shop.example/catalog/toy/item.php?id=88",
        "88"
    )]
    #[case::code_with_a_malformed_subtag(
        "https://shop.example/store/en-x/print/88",
        "https://shop.example/store/print/88",
        "88"
    )]
    #[case::other_page_name(
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689",
        "https://hara.example/english/book_detail.php?print_id=24689",
        "24689"
    )]
    #[case::two_differing_directories(
        "https://hara.example/ukiyoe/prints/detail.php?print_id=24689",
        "https://hara.example/english/books/detail.php?print_id=24689",
        "24689"
    )]
    #[case::ordinary_directory_dropped(
        "https://hara.example/english/ukiyoe/detail.php?print_id=24689",
        "https://hara.example/detail.php?print_id=24689",
        "24689"
    )]
    #[case::language_named_page(
        "https://shop.example/prints/24689/japanese",
        "https://shop.example/prints/24689/english",
        "24689"
    )]
    #[case::other_host(
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689",
        "https://other.example/english/ukiyoe_detail.php?print_id=24689",
        "24689"
    )]
    #[case::one_spelling(
        "https://shop.example/english/print.php?id=88",
        "https://shop.example/english/print.php?id=88",
        "88"
    )]
    fn should_not_take_other_pages_for_language_versions(
        #[case] url: &str,
        #[case] other: &str,
        #[case] identity: &str,
    ) {
        assert!(!language_versions(url, other, identity));
    }

    #[rstest::rstest]
    #[case::code("de", true)]
    #[case::upper_case_code("FR", true)]
    #[case::code_with_region("pt-br", true)]
    #[case::code_with_script("zh_Hans", true)]
    #[case::name("english", true)]
    #[case::capitalized_name("Deutsch", true)]
    #[case::no_iso_639_1_code("xx", false)]
    #[case::three_letters("art", false)]
    #[case::long_subtag("en-british", false)]
    #[case::word("prints", false)]
    fn should_tell_whether_a_directory_names_a_language(
        #[case] directory: &str,
        #[case] names: bool,
    ) {
        assert_eq!(is_language_directory(directory), names);
    }
}
