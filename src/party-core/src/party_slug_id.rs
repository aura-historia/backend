use crate::party_name::PartyName;
use std::{fmt::Display, iter, ops::Deref};

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid party slug '{value}'")]
pub struct InvalidPartySlugId {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartySlugId(String);

impl PartySlugId {
    pub const FALLBACK_PREFIX: &str = "party";

    pub fn raw(value: impl AsRef<str>) -> Result<Self, InvalidPartySlugId> {
        let value = value.as_ref();
        if !value.is_empty()
            && !value.starts_with('-')
            && !value.ends_with('-')
            && !value.contains("--")
            && value.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidPartySlugId {
                value: value.to_owned(),
            })
        }
    }

    /// Slugs a new Party named `name` may take, in preference order: the slugified name
    /// ([`Self::FALLBACK_PREFIX`] when nothing of it survives slugification), then that base
    /// followed by `-2`, `-3` and so on for when the earlier candidates are already taken.
    pub fn candidates(name: &PartyName) -> impl Iterator<Item = Self> + use<> {
        let base = match slug::slugify(name.as_ref()) {
            base if base.is_empty() => Self::FALLBACK_PREFIX.to_owned(),
            base => base,
        };
        let disambiguated = (2_u32..).map({
            let base = base.clone();
            move |ordinal| Self(format!("{base}-{ordinal}"))
        });
        iter::once(Self(base)).chain(disambiguated)
    }
}

impl Display for PartySlugId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PartySlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for PartySlugId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(name: &str, count: usize) -> Vec<String> {
        let name = PartyName::try_from(name)
            .unwrap_or_else(|error| panic!("invalid test party name: {error}"));
        PartySlugId::candidates(&name)
            .take(count)
            .map(|slug_id| slug_id.to_string())
            .collect()
    }

    #[test]
    fn should_offer_slugified_name_then_numbered_variants() {
        assert_eq!(
            vec!["antik-und-stil", "antik-und-stil-2", "antik-und-stil-3"],
            candidates("  Antik und Stil ", 3)
        );
    }

    #[test]
    fn should_fall_back_to_party_base_when_name_has_no_slug_characters() {
        assert_eq!(vec!["party", "party-2"], candidates("\u{10FFFF}", 2));
    }

    #[test]
    fn should_offer_only_valid_persistable_slugs() {
        for candidate in candidates("Müller & Söhne – Antiquitäten", 3) {
            assert_eq!(
                Ok(candidate.as_str()),
                PartySlugId::raw(&candidate).as_deref()
            );
        }
    }
}
