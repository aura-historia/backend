//! Explicit item statements, never translations or substring guesses at runtime.
//!
//! One module per script family keeps each file reviewable; a language is a `LanguagePhrases`
//! constant listed once in [`LANGUAGES`].
mod east_asian;
mod latin;
mod other_scripts;

use localization::Language;

pub(super) struct LanguagePhrases {
    pub language: Language,
    /// Whole statements, one per line.
    pub prohibited: &'static str,
    pub collection_only: &'static str,
    pub worldwide: &'static str,
    pub on_request: &'static str,
    pub calculated: &'static str,
    /// Prefix/suffix include their own whitespace. Every complete expression is anchored.
    pub dispatch_prefix: &'static str,
    pub range_separator: &'static str,
    pub dispatch_suffix: &'static str,
    /// Word prefixes are used only to abstain on unrecognized delivery clauses.
    pub delivery_words: &'static [&'static str],
    /// For scripts without word separators; these markers can only veto an assertion.
    pub delivery_markers: &'static [&'static str],
    pub conditions: &'static [&'static str],
    pub condition_markers: &'static [&'static str],
}

pub(super) const LANGUAGES: &[LanguagePhrases] = &[
    latin::EN,
    latin::DE,
    latin::FR,
    latin::ES,
    latin::IT,
    east_asian::ZH,
    latin::PT,
    latin::PL,
    latin::TR,
    latin::NL,
    latin::CS,
    east_asian::JA,
    other_scripts::RU,
    other_scripts::AR,
    east_asian::KO,
    other_scripts::EL,
    other_scripts::HE,
    other_scripts::TH,
    other_scripts::HI,
];
