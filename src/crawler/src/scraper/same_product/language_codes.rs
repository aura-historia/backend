//! The standard codes by which a URL path names a language
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`).

/// The ISO 639-1 language codes.
const LANGUAGE_CODES: &[&str] = &[
    "aa", "ab", "ae", "af", "ak", "am", "an", "ar", "as", "av", "ay", "az", "ba", "be", "bg", "bh",
    "bi", "bm", "bn", "bo", "br", "bs", "ca", "ce", "ch", "co", "cr", "cs", "cu", "cv", "cy", "da",
    "de", "dv", "dz", "ee", "el", "en", "eo", "es", "et", "eu", "fa", "ff", "fi", "fj", "fo", "fr",
    "fy", "ga", "gd", "gl", "gn", "gu", "gv", "ha", "he", "hi", "ho", "hr", "ht", "hu", "hy", "hz",
    "ia", "id", "ie", "ig", "ii", "ik", "io", "is", "it", "iu", "ja", "jv", "ka", "kg", "ki", "kj",
    "kk", "kl", "km", "kn", "ko", "kr", "ks", "ku", "kv", "kw", "ky", "la", "lb", "lg", "li", "ln",
    "lo", "lt", "lu", "lv", "mg", "mh", "mi", "mk", "ml", "mn", "mr", "ms", "mt", "my", "na", "nb",
    "nd", "ne", "ng", "nl", "nn", "no", "nr", "nv", "ny", "oc", "oj", "om", "or", "os", "pa", "pi",
    "pl", "ps", "pt", "qu", "rm", "rn", "ro", "ru", "rw", "sa", "sc", "sd", "se", "sg", "si", "sk",
    "sl", "sm", "sn", "so", "sq", "sr", "ss", "st", "su", "sv", "sw", "ta", "te", "tg", "th", "ti",
    "tk", "tl", "tn", "to", "tr", "ts", "tt", "tw", "ty", "ug", "uk", "ur", "uz", "ve", "vi", "vo",
    "wa", "wo", "xh", "yi", "yo", "za", "zh", "zu",
];

/// The ISO 639-2 codes of the ISO 639-1 languages, terminology and bibliographic (`deu` and
/// `ger`), by which a shop may name a language version before its path (`/eng/`, `/ita/`).
const THREE_LETTER_LANGUAGE_CODES: &[&str] = &[
    "aar", "abk", "afr", "aka", "alb", "amh", "ara", "arg", "arm", "asm", "ava", "ave", "aym",
    "aze", "bak", "bam", "baq", "bel", "ben", "bih", "bis", "bod", "bos", "bre", "bul", "bur",
    "cat", "ces", "cha", "che", "chi", "chu", "chv", "cor", "cos", "cre", "cym", "cze", "dan",
    "deu", "div", "dut", "dzo", "ell", "eng", "epo", "est", "eus", "ewe", "fao", "fas", "fij",
    "fin", "fra", "fre", "fry", "ful", "geo", "ger", "gla", "gle", "glg", "glv", "gre", "grn",
    "guj", "hat", "hau", "heb", "her", "hin", "hmo", "hrv", "hun", "hye", "ibo", "ice", "ido",
    "iii", "iku", "ile", "ina", "ind", "ipk", "isl", "ita", "jav", "jpn", "kal", "kan", "kas",
    "kat", "kau", "kaz", "khm", "kik", "kin", "kir", "kom", "kon", "kor", "kua", "kur", "lao",
    "lat", "lav", "lim", "lin", "lit", "ltz", "lub", "lug", "mac", "mah", "mal", "mao", "mar",
    "may", "mkd", "mlg", "mlt", "mon", "mri", "msa", "mya", "nau", "nav", "nbl", "nde", "ndo",
    "nep", "nld", "nno", "nob", "nor", "nya", "oci", "oji", "ori", "orm", "oss", "pan", "per",
    "pli", "pol", "por", "pus", "que", "roh", "ron", "rum", "run", "rus", "sag", "san", "sin",
    "slk", "slo", "slv", "sme", "smo", "sna", "snd", "som", "sot", "spa", "sqi", "srd", "srp",
    "ssw", "sun", "swa", "swe", "tah", "tam", "tat", "tel", "tgk", "tgl", "tha", "tib", "tir",
    "ton", "tsn", "tso", "tuk", "tur", "twi", "uig", "ukr", "urd", "uzb", "ven", "vie", "vol",
    "wel", "wln", "wol", "xho", "yid", "yor", "zha", "zho", "zul",
];

/// The ISO 3166-1 alpha-2 country codes, by which a shop may name a regional version before its
/// path (`/us/`, `/gb/`, `/us-en/`).
const COUNTRY_CODES: &[&str] = &[
    "ad", "ae", "af", "ag", "ai", "al", "am", "ao", "aq", "ar", "as", "at", "au", "aw", "ax", "az",
    "ba", "bb", "bd", "be", "bf", "bg", "bh", "bi", "bj", "bl", "bm", "bn", "bo", "bq", "br", "bs",
    "bt", "bv", "bw", "by", "bz", "ca", "cc", "cd", "cf", "cg", "ch", "ci", "ck", "cl", "cm", "cn",
    "co", "cr", "cu", "cv", "cw", "cx", "cy", "cz", "de", "dj", "dk", "dm", "do", "dz", "ec", "ee",
    "eg", "eh", "er", "es", "et", "fi", "fj", "fk", "fm", "fo", "fr", "ga", "gb", "gd", "ge", "gf",
    "gg", "gh", "gi", "gl", "gm", "gn", "gp", "gq", "gr", "gs", "gt", "gu", "gw", "gy", "hk", "hm",
    "hn", "hr", "ht", "hu", "id", "ie", "il", "im", "in", "io", "iq", "ir", "is", "it", "je", "jm",
    "jo", "jp", "ke", "kg", "kh", "ki", "km", "kn", "kp", "kr", "kw", "ky", "kz", "la", "lb", "lc",
    "li", "lk", "lr", "ls", "lt", "lu", "lv", "ly", "ma", "mc", "md", "me", "mf", "mg", "mh", "mk",
    "ml", "mm", "mn", "mo", "mp", "mq", "mr", "ms", "mt", "mu", "mv", "mw", "mx", "my", "mz", "na",
    "nc", "ne", "nf", "ng", "ni", "nl", "no", "np", "nr", "nu", "nz", "om", "pa", "pe", "pf", "pg",
    "ph", "pk", "pl", "pm", "pn", "pr", "ps", "pt", "pw", "py", "qa", "re", "ro", "rs", "ru", "rw",
    "sa", "sb", "sc", "sd", "se", "sg", "sh", "si", "sj", "sk", "sl", "sm", "sn", "so", "sr", "ss",
    "st", "sv", "sx", "sy", "sz", "tc", "td", "tf", "tg", "th", "tj", "tk", "tl", "tm", "tn", "to",
    "tr", "tt", "tv", "tw", "tz", "ua", "ug", "um", "us", "uy", "uz", "va", "vc", "ve", "vg", "vi",
    "vn", "vu", "wf", "ws", "ye", "yt", "za", "zm", "zw",
];

/// Whether `directory` is an ISO 639-1 code ([`LANGUAGE_CODES`]), alone or with a region or
/// script subtag (`de`, `pt-br`, `zh_hans`).
pub(super) fn is_language_code(directory: &str) -> bool {
    code_of(directory).is_some_and(|code| is_listed(LANGUAGE_CODES, code))
}

/// Whether `directory` names a locale as a shop puts it before a path: a language code
/// ([`LANGUAGE_CODES`], [`THREE_LETTER_LANGUAGE_CODES`]) or a country code ([`COUNTRY_CODES`]),
/// alone or with a subtag (`en`, `eng`, `us`, `de-at`, `us-en`, `pt_BR`). Two or three letters
/// that name no language or country, such as `art`, `map` or `new`, are a section of the shop.
pub(super) fn is_locale(directory: &str) -> bool {
    code_of(directory).is_some_and(|code| {
        [LANGUAGE_CODES, THREE_LETTER_LANGUAGE_CODES, COUNTRY_CODES]
            .iter()
            .any(|codes| is_listed(codes, code))
    })
}

/// The code `directory` starts with, when what follows it is no subtag or a subtag of two to four
/// letters or digits (`pt` for `pt-br`); `None` otherwise.
fn code_of(directory: &str) -> Option<&str> {
    match directory.split_once(['-', '_']) {
        None => Some(directory),
        Some((code, subtag)) => ((2..=4).contains(&subtag.len())
            && subtag.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .then_some(code),
    }
}

fn is_listed(codes: &[&str], code: &str) -> bool {
    codes.iter().any(|listed| code.eq_ignore_ascii_case(listed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_hold_the_184_iso_639_1_codes_once_each() {
        let mut codes = LANGUAGE_CODES.to_vec();
        codes.sort_unstable();
        codes.dedup();

        assert_eq!(codes.len(), 184);
        assert_eq!(codes.len(), LANGUAGE_CODES.len());
    }

    /// The terminology code of each ISO 639-1 language, and the bibliographic one of the 20 that
    /// have another; the 249 ISO 3166-1 alpha-2 countries.
    #[rstest::rstest]
    #[case::three_letter_language_codes(THREE_LETTER_LANGUAGE_CODES, 204)]
    #[case::country_codes(COUNTRY_CODES, 249)]
    fn should_hold_each_standard_code_once(#[case] codes: &[&str], #[case] count: usize) {
        let mut unique = codes.to_vec();
        unique.sort_unstable();
        unique.dedup();

        assert_eq!(unique.len(), count);
        assert_eq!(unique.len(), codes.len());
    }

    #[rstest::rstest]
    #[case::language("en", true)]
    #[case::language_with_region("de-AT", true)]
    #[case::language_with_script("zh_hans", true)]
    #[case::three_letter_language("eng", true)]
    #[case::bibliographic_language("ger", true)]
    #[case::country("us", true)]
    #[case::country_with_language("us-en", true)]
    #[case::section("art", false)]
    #[case::other_section("map", false)]
    #[case::word("new", false)]
    #[case::no_code("xx", false)]
    #[case::long_subtag("en-british", false)]
    #[case::empty_subtag("en-", false)]
    #[case::four_letters("engl", false)]
    fn should_tell_whether_a_directory_names_a_locale(
        #[case] directory: &str,
        #[case] locale: bool,
    ) {
        assert_eq!(is_locale(directory), locale);
    }
}
