//! Whether two product URLs name one page in two language versions
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`): their directories differ in one
//! language directory, and the product identity both pages show is part of what the URLs share. A
//! language code before the path is already a spelling
//! ([`product_spelling`](super::product_spelling)); this rule covers a language directory at any
//! depth. A language code inserted or dropped shows a language version by itself; a language named
//! in words inserted or dropped, and any directory swapped for another one, do only when the two
//! titles are written in different writing systems, since the words and codes that name languages
//! also name origins, sections and dealers.

use super::language_codes::is_language_code;
use url::Url;

/// Languages named in words, in English and in the language itself without diacritics, as a shop
/// names the directory of a language version (`/english/…`, `/deutsch/…`). The same words name
/// an antique's origin (`/chinese/` porcelain, `/english/` furniture), so such a directory, inserted
/// or swapped for another one, shows a language version only with titles in two writing systems.
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

/// A page as the language-version rule compares it.
#[derive(Debug, Clone, Copy)]
pub(super) struct VersionedPage<'a> {
    /// The URL's [page spelling](super::page_spelling).
    pub(super) spelling: &'a Url,
    /// The page's title, as the same-product rule knows it ([`super::ProductUrlIdentity::title`]).
    pub(super) title: Option<&'a str>,
}

/// Whether `page` and `other` name one page in two language versions: on one origin, with one
/// query and one page name, their directories differ in one language directory
/// ([`LanguageDifference`]), a language named in words or a swapped directory only with titles in
/// two writing systems ([`are_in_different_writing_systems`]), and `identity` is a whole query value, a directory both
/// keep or the page name, so the URLs themselves name the product the shared identity stands for.
/// A language directory is never the identity's token.
pub(super) fn are_language_versions(
    page: VersionedPage<'_>,
    other: VersionedPage<'_>,
    identity: &str,
) -> bool {
    let (spelling, other_spelling) = (page.spelling, other.spelling);
    if identity.is_empty()
        || spelling.origin() != other_spelling.origin()
        || spelling.query() != other_spelling.query()
    {
        return false;
    }
    let (Some(path), Some(other_path)) = (PagePath::of(spelling), PagePath::of(other_spelling))
    else {
        return false;
    };
    if path.name != other_path.name {
        return false;
    }
    let titles_differ = || are_in_different_writing_systems(page.title, other.title);
    let shared = match path.language_difference(&other_path) {
        Some(LanguageDifference::Inserted { shared, directory })
            if is_language_code(directory) || titles_differ() =>
        {
            shared
        }
        Some(LanguageDifference::Swapped { shared }) if titles_differ() => shared,
        _ => return false,
    };
    path.name == identity
        || shared.contains(&identity)
        || query_values(spelling.query()).any(|value| value == identity)
}

/// A URL path as its directories and the name of the page they hold.
struct PagePath<'a> {
    directories: Vec<&'a str>,
    name: &'a str,
}

/// How two paths' directories differ when they differ in one language directory, with the
/// directories both keep, in order.
#[derive(Debug, PartialEq, Eq)]
enum LanguageDifference<'a> {
    /// A language `directory` ([`is_language_directory`]) inserted into, or dropped from,
    /// otherwise equal directories (`/shop/de/item` and `/shop/item`).
    Inserted {
        shared: Vec<&'a str>,
        directory: &'a str,
    },
    /// One directory swapped for another at the same place, at least one of them a language
    /// directory (`/ukiyoe/detail` and `/english/detail`).
    Swapped { shared: Vec<&'a str> },
}

impl<'a> PagePath<'a> {
    fn of(url: &'a Url) -> Option<Self> {
        let mut directories: Vec<&str> = url.path_segments()?.collect();
        let name = directories.pop()?;
        Some(Self { directories, name })
    }

    /// How these directories differ from `other`'s in one language directory; `None` for equal
    /// directories, which are one spelling, and for any other difference, such as two directories,
    /// a reordering or an ordinary directory inserted.
    fn language_difference(&self, other: &Self) -> Option<LanguageDifference<'a>> {
        let (mine, theirs) = (&self.directories, &other.directories);
        if mine.len() == theirs.len() {
            let mut differing = (0..mine.len()).filter(|&index| mine[index] != theirs[index]);
            return match (differing.next(), differing.next()) {
                (Some(index), None)
                    if is_language_directory(mine[index])
                        || is_language_directory(theirs[index]) =>
                {
                    let mut shared = mine.clone();
                    shared.remove(index);
                    Some(LanguageDifference::Swapped { shared })
                }
                _ => None,
            };
        }
        let (longer, shorter) = if mine.len() > theirs.len() {
            (mine, theirs)
        } else {
            (theirs, mine)
        };
        if longer.len() != shorter.len() + 1 {
            return None;
        }
        let index = (0..shorter.len())
            .find(|&index| longer[index] != shorter[index])
            .unwrap_or(shorter.len());
        (is_language_directory(longer[index]) && longer[index + 1..] == shorter[index..]).then(
            || LanguageDifference::Inserted {
                shared: shorter.clone(),
                directory: longer[index],
            },
        )
    }
}

/// Whether a directory names a language: a language named in words ([`LANGUAGE_NAMES`]) or an
/// ISO 639-1 code ([`is_language_code`]) with an optional region or script subtag.
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

/// The writing systems a title's letters are told apart by. Chinese and Japanese share the Han
/// characters, so they are one system here: a Japanese and a Chinese title tell nothing apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WritingSystem {
    Latin,
    Greek,
    Cyrillic,
    Hebrew,
    Arabic,
    Devanagari,
    Thai,
    Hangul,
    HanAndKana,
}

impl WritingSystem {
    const ALL: [Self; 9] = [
        Self::Latin,
        Self::Greek,
        Self::Cyrillic,
        Self::Hebrew,
        Self::Arabic,
        Self::Devanagari,
        Self::Thai,
        Self::Hangul,
        Self::HanAndKana,
    ];

    fn of(character: char) -> Option<Self> {
        if !character.is_alphabetic() {
            return None;
        }
        match u32::from(character) {
            0x0041..=0x024F | 0x1E00..=0x1EFF => Some(Self::Latin),
            0x0370..=0x03FF | 0x1F00..=0x1FFF => Some(Self::Greek),
            0x0400..=0x052F => Some(Self::Cyrillic),
            0x0590..=0x05FF => Some(Self::Hebrew),
            0x0600..=0x06FF | 0x0750..=0x077F => Some(Self::Arabic),
            0x0900..=0x097F => Some(Self::Devanagari),
            0x0E00..=0x0E7F => Some(Self::Thai),
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Some(Self::Hangul),
            0x3040..=0x30FF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF => Some(Self::HanAndKana),
            _ => None,
        }
    }

    /// The writing system most of `title`'s letters are in; `None` for a title without letters
    /// of a known system, such as a bare number, and for a tie between two systems.
    fn main_of(title: &str) -> Option<Self> {
        let mut counts = [0_usize; Self::ALL.len()];
        for system in title.chars().filter_map(Self::of) {
            if let Some(index) = Self::ALL.iter().position(|known| *known == system) {
                counts[index] += 1;
            }
        }
        let most = counts.iter().copied().max()?;
        let mut leading = (0..counts.len()).filter(|&index| counts[index] == most);
        match (leading.next(), leading.next()) {
            (Some(index), None) if most > 0 => Some(Self::ALL[index]),
            _ => None,
        }
    }
}

/// Whether two titles are written in different writing systems (a Japanese and an English title),
/// the sign that two pages swapping a directory are one product in two languages rather than two
/// products of one shop's sections (`/chinese/` and `/japanese/`, titled in the shop's language).
/// A missing title, or one without letters, shows nothing.
fn are_in_different_writing_systems(title: Option<&str>, other: Option<&str>) -> bool {
    let systems = title
        .and_then(WritingSystem::main_of)
        .zip(other.and_then(WritingSystem::main_of));
    systems.is_some_and(|(system, other)| system != other)
}

#[cfg(test)]
mod tests {
    use super::super::page_spelling;
    use super::*;

    const JAPANESE: Option<&str> = Some("豊国三代　時世桜狩図");
    const ENGLISH: Option<&str> = Some("TOYOKUNI III Beauties by a teahouse in a cherry garden");

    fn language_versions(
        (url, title): (&str, Option<&str>),
        (other, other_title): (&str, Option<&str>),
        identity: &str,
    ) -> bool {
        let (spelling, other_spelling) = (
            page_spelling(&Url::parse(url).unwrap()),
            page_spelling(&Url::parse(other).unwrap()),
        );
        are_language_versions(
            VersionedPage {
                spelling: &spelling,
                title,
            },
            VersionedPage {
                spelling: &other_spelling,
                title: other_title,
            },
            identity,
        )
    }

    /// Hara Shobo serves each print in Japanese under `/ukiyoe/` and in English under `/english/`;
    /// other shops insert a language code or name at any depth, or swap one for another.
    #[rstest::rstest]
    #[case::directory_swapped_for_a_language_name(
        ("https://hara.example/ukiyoe//ukiyoe_detail.php?print_id=24689", JAPANESE),
        ("https://hara.example/english/ukiyoe_detail.php?print_id=24689", ENGLISH),
        "24689"
    )]
    #[case::language_name_swapped_back(
        ("https://hara.example/english/ukiyoe_detail.php?print_id=24689", ENGLISH),
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689", JAPANESE),
        "24689"
    )]
    #[case::tracking_parameter(
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689&utm_source=mail", JAPANESE),
        ("https://hara.example/english/ukiyoe_detail.php?print_id=24689", ENGLISH),
        "24689"
    )]
    #[case::two_language_names(
        ("https://shop.example/Japanese/prints/24689", JAPANESE),
        ("https://shop.example/english/prints/24689/", ENGLISH),
        "24689"
    )]
    #[case::two_codes_with_subtags(
        ("https://shop.example/store/ja-JP/print/88", Some("伊万里大皿")),
        ("https://shop.example/store/en_gb/print/88", Some("Imari charger")),
        "88"
    )]
    #[case::code_inserted_inside_the_path(
        ("https://shop.example/catalogue/fr/print/88", Some("Plat Imari")),
        ("https://shop.example/catalogue/print/88", Some("Imari charger")),
        "88"
    )]
    #[case::name_inserted_with_titles_in_two_writing_systems(
        ("https://shop.example/catalogue/english/print.php?id=88", ENGLISH),
        ("https://shop.example/catalogue/print.php?id=88", JAPANESE),
        "88"
    )]
    #[case::code_inserted_before_the_path(
        ("https://shop.example/en/catalogue/print.php?id=88", None),
        ("https://shop.example/catalogue/print.php?id=88", None),
        "88"
    )]
    #[case::identity_as_the_page_name(
        ("https://shop.example/ja/prints/hiroshige-88", None),
        ("https://shop.example/prints/hiroshige-88", None),
        "hiroshige-88"
    )]
    fn should_take_two_language_versions_of_one_page(
        #[case] page: (&str, Option<&str>),
        #[case] other: (&str, Option<&str>),
        #[case] identity: &str,
    ) {
        assert!(language_versions(page, other, identity));
    }

    /// The words and codes that name languages also name an antique's origin, a section of the
    /// shop or a dealer, each of which may number its products on its own: a swapped directory
    /// shows nothing without titles in two writing systems, and only one directory may differ.
    #[rstest::rstest]
    #[case::origin_categories_swapped(
        ("https://shop.example/chinese/item.php?id=12", Some("Famille rose vase")),
        ("https://shop.example/japanese/item.php?id=12", Some("Imari dish")),
        "12"
    )]
    #[case::origin_categories_inside_a_section(
        ("https://shop.example/porcelain/chinese/item.php?id=12", Some("Famille rose vase")),
        ("https://shop.example/porcelain/japanese/item.php?id=12", Some("Imari dish")),
        "12"
    )]
    #[case::origin_category_for_a_section(
        ("https://shop.example/furniture/item.php?id=12", Some("Oak coffer")),
        ("https://shop.example/english/item.php?id=12", Some("Walnut chest")),
        "12"
    )]
    #[case::han_titles_on_both_sides(
        ("https://shop.example/chinese/item.php?id=12", Some("粉彩花瓶")),
        ("https://shop.example/japanese/item.php?id=12", Some("伊万里皿")),
        "12"
    )]
    #[case::dealer_initials_that_are_language_codes(
        ("https://shop.example/dealers/ms/item.php?id=12", Some("Regency card table")),
        ("https://shop.example/dealers/ab/item.php?id=12", Some("Pair of candlesticks")),
        "12"
    )]
    #[case::locations_that_are_language_codes(
        ("https://shop.example/stock/ny/item?id=5", Some("Tiffany lamp")),
        ("https://shop.example/stock/la/item?id=5", Some("Art deco mirror")),
        "5"
    )]
    #[case::origin_category_inserted(
        ("https://shop.example/prints/japanese/detail.php?id=5", Some("Imari dish")),
        ("https://shop.example/prints/detail.php?id=5", Some("Kuniyoshi warrior print")),
        "5"
    )]
    #[case::origin_category_inserted_before_the_path(
        ("https://shop.example/chinese/item.php?id=12", Some("Famille rose vase")),
        ("https://shop.example/item.php?id=12", Some("Oak coffer")),
        "12"
    )]
    #[case::name_inserted_without_titles(
        ("https://shop.example/catalogue/english/print.php?id=88", None),
        ("https://shop.example/catalogue/print.php?id=88", None),
        "88"
    )]
    #[case::swap_with_a_missing_title(
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689", JAPANESE),
        ("https://hara.example/english/ukiyoe_detail.php?print_id=24689", None),
        "24689"
    )]
    #[case::directories_reordered(
        ("https://shop.example/a/en/b/x?id=1", None),
        ("https://shop.example/a/b/de/x?id=1", None),
        "1"
    )]
    #[case::code_in_place_of_a_category(
        ("https://shop.example/prints/vases/item.php?id=88", Some("Celadon vase")),
        ("https://shop.example/prints/de/item.php?id=88", Some("Seladon-Vase")),
        "88"
    )]
    #[case::identity_not_in_the_urls(
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689", JAPANESE),
        ("https://hara.example/english/ukiyoe_detail.php?print_id=24689", ENGLISH),
        "J263"
    )]
    #[case::identity_part_of_a_value(
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=124689", JAPANESE),
        ("https://hara.example/english/ukiyoe_detail.php?print_id=124689", ENGLISH),
        "24689"
    )]
    #[case::identity_only_the_language_directory(
        ("https://shop.example/catalogue/english/print.php?id=88", None),
        ("https://shop.example/catalogue/print.php?id=88", None),
        "english"
    )]
    #[case::other_query(
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689&set=2", JAPANESE),
        ("https://hara.example/english/ukiyoe_detail.php?print_id=24689", ENGLISH),
        "24689"
    )]
    #[case::no_language_directory(
        ("https://shop.example/vases/item.php?id=88", JAPANESE),
        ("https://shop.example/bowls/item.php?id=88", ENGLISH),
        "88"
    )]
    #[case::three_letters_no_language_code(
        ("https://shop.example/catalog/art/item.php?id=88", None),
        ("https://shop.example/catalog/item.php?id=88", None),
        "88"
    )]
    #[case::code_with_a_malformed_subtag(
        ("https://shop.example/store/en-x/print/88", None),
        ("https://shop.example/store/print/88", None),
        "88"
    )]
    #[case::other_page_name(
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689", JAPANESE),
        ("https://hara.example/english/book_detail.php?print_id=24689", ENGLISH),
        "24689"
    )]
    #[case::two_differing_directories(
        ("https://hara.example/ukiyoe/prints/detail.php?print_id=24689", JAPANESE),
        ("https://hara.example/english/books/detail.php?print_id=24689", ENGLISH),
        "24689"
    )]
    #[case::ordinary_directory_dropped_too(
        ("https://hara.example/english/ukiyoe/detail.php?print_id=24689", ENGLISH),
        ("https://hara.example/detail.php?print_id=24689", JAPANESE),
        "24689"
    )]
    #[case::language_named_page(
        ("https://shop.example/prints/24689/japanese", JAPANESE),
        ("https://shop.example/prints/24689/english", ENGLISH),
        "24689"
    )]
    #[case::other_host(
        ("https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=24689", JAPANESE),
        ("https://other.example/english/ukiyoe_detail.php?print_id=24689", ENGLISH),
        "24689"
    )]
    #[case::one_spelling(
        ("https://shop.example/english/print.php?id=88", JAPANESE),
        ("https://shop.example/english/print.php?id=88", ENGLISH),
        "88"
    )]
    fn should_not_take_other_pages_for_language_versions(
        #[case] page: (&str, Option<&str>),
        #[case] other: (&str, Option<&str>),
        #[case] identity: &str,
    ) {
        assert!(!language_versions(page, other, identity));
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

    #[rstest::rstest]
    #[case::english("Imari charger, circa 1700", Some(WritingSystem::Latin))]
    #[case::accented_latin("Commode à la française", Some(WritingSystem::Latin))]
    #[case::japanese_with_kana_and_digits(
        "豊国三代　時世桜狩図 (1860)",
        Some(WritingSystem::HanAndKana)
    )]
    #[case::chinese("粉彩花瓶", Some(WritingSystem::HanAndKana))]
    #[case::korean("백자 달항아리", Some(WritingSystem::Hangul))]
    #[case::russian("Икона Казанской Богоматери", Some(WritingSystem::Cyrillic))]
    #[case::mostly_japanese_with_a_latin_name(
        "歌川国芳 Kuniyoshi 武者絵 大判三枚続",
        Some(WritingSystem::HanAndKana)
    )]
    #[case::bare_number("1893", None)]
    #[case::tie_between_two_systems("Hara 原書房画", None)]
    fn should_tell_the_main_writing_system_of_a_title(
        #[case] title: &str,
        #[case] system: Option<WritingSystem>,
    ) {
        assert_eq!(WritingSystem::main_of(title), system);
    }

    #[rstest::rstest]
    #[case::japanese_and_english(JAPANESE, ENGLISH, true)]
    #[case::two_latin_languages(Some("Imari charger"), Some("Imari-Teller"), false)]
    #[case::chinese_and_japanese(Some("粉彩花瓶"), Some("伊万里皿"), false)]
    #[case::missing_title(JAPANESE, None, false)]
    #[case::title_without_letters(JAPANESE, Some("1860"), false)]
    fn should_tell_whether_two_titles_are_in_different_writing_systems(
        #[case] title: Option<&str>,
        #[case] other: Option<&str>,
        #[case] different: bool,
    ) {
        assert_eq!(are_in_different_writing_systems(title, other), different);
    }
}
