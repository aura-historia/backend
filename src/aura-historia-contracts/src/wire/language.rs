use localization::Language;
use serde::{Deserializer, Serializer};

const LOCALE_ALIASES: &[(&str, Language)] = &[
    ("de-DE", Language::De),
    ("de-AT", Language::De),
    ("de-CH", Language::De),
    ("de-LU", Language::De),
    ("de-LI", Language::De),
    ("en-US", Language::En),
    ("en-GB", Language::En),
    ("en-AU", Language::En),
    ("en-CA", Language::En),
    ("en-NZ", Language::En),
    ("en_IE", Language::En),
    ("fr-FR", Language::Fr),
    ("fr-CA", Language::Fr),
    ("fr-BE", Language::Fr),
    ("fr-CH", Language::Fr),
    ("fr-LU", Language::Fr),
    ("es-ES", Language::Es),
    ("es-MX", Language::Es),
    ("es-AR", Language::Es),
    ("es-CO", Language::Es),
    ("es-CL", Language::Es),
    ("es-PE", Language::Es),
    ("es-VE", Language::Es),
    ("it-IT", Language::It),
    ("it-CH", Language::It),
    ("zh-CN", Language::Zh),
    ("zh-Hans", Language::Zh),
    ("pt-PT", Language::Pt),
    ("pt-BR", Language::Pt),
    ("pl-PL", Language::Pl),
    ("tr-TR", Language::Tr),
    ("nl-NL", Language::Nl),
    ("nl-BE", Language::Nl),
    ("cs-CZ", Language::Cs),
    ("ja-JP", Language::Ja),
    ("ru-RU", Language::Ru),
    ("ar-SA", Language::Ar),
    ("ar-EG", Language::Ar),
    ("ar-AE", Language::Ar),
];

pub fn parse(value: &str) -> Option<Language> {
    LOCALE_ALIASES
        .iter()
        .find_map(|(alias, language)| (*alias == value).then_some(*language))
        .or_else(|| Language::from_code(value))
}

pub fn serialize<S>(value: &Language, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(value.as_str())
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Language, D::Error>
where
    D: Deserializer<'de>,
{
    super::deserialize_code(deserializer, parse)
}
