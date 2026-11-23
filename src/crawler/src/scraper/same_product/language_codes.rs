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

/// Whether `directory` is an ISO 639-1 code ([`LANGUAGE_CODES`]), alone or with a region or
/// script subtag (`de`, `pt-br`, `zh_hans`).
pub(super) fn is_language_code(directory: &str) -> bool {
    let (code, subtag) = directory
        .split_once(['-', '_'])
        .map_or((directory, None), |(code, subtag)| (code, Some(subtag)));
    let subtag_fits = subtag.is_none_or(|subtag| {
        (2..=4).contains(&subtag.len()) && subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
    });
    subtag_fits
        && LANGUAGE_CODES
            .iter()
            .any(|language| code.eq_ignore_ascii_case(language))
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
}
