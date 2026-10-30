use std::{
    fmt::{Display, Formatter},
    iter,
};

use crate::ListingSourceName;

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid listing source slug '{value}'")]
pub struct InvalidListingSourceSlug {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingSourceSlugId(String);

impl ListingSourceSlugId {
    const FALLBACK_BASE: &str = "listing-source";

    pub fn raw(value: impl AsRef<str>) -> Result<Self, InvalidListingSourceSlug> {
        let value = value.as_ref();
        if !value.is_empty()
            && !value.starts_with('-')
            && !value.ends_with('-')
            && !value.contains("--")
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidListingSourceSlug {
                value: value.to_owned(),
            })
        }
    }

    /// Slugs a new ListingSource named `name` may take, in preference order: the slugified name
    /// (`listing-source` when nothing of it survives slugification), then that base followed by `-2`,
    /// `-3` and so on for when the earlier candidates are already taken.
    pub fn candidates(name: &ListingSourceName) -> impl Iterator<Item = Self> + use<> {
        let base = match slug::slugify(name.as_ref()) {
            base if base.is_empty() => Self::FALLBACK_BASE.to_owned(),
            base => base,
        };
        let disambiguated = (2_u32..).map({
            let base = base.clone();
            move |ordinal| Self(format!("{base}-{ordinal}"))
        });
        iter::once(Self(base)).chain(disambiguated)
    }
}

impl AsRef<str> for ListingSourceSlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for ListingSourceSlugId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(name: &str, count: usize) -> Vec<String> {
        let name = ListingSourceName::try_from(name)
            .unwrap_or_else(|error| panic!("invalid test listing source name: {error}"));
        ListingSourceSlugId::candidates(&name)
            .take(count)
            .map(|slug_id| slug_id.to_string())
            .collect()
    }

    #[test]
    fn should_offer_slugified_name_then_numbered_variants() {
        assert_eq!(
            vec!["antik-und-stil", "antik-und-stil-2", "antik-und-stil-3"],
            candidates("Antik und Stil", 3)
        );
    }

    #[test]
    fn should_fall_back_to_listing_source_base_when_name_has_no_slug_characters() {
        assert_eq!(
            vec!["listing-source", "listing-source-2"],
            candidates("\u{10FFFF}", 2)
        );
    }

    #[test]
    fn should_append_ordinal_even_when_slugified_name_ends_in_digits() {
        assert_eq!(vec!["shop-2", "shop-2-2"], candidates("Shop 2", 2));
    }

    #[test]
    fn should_offer_only_valid_persistable_slugs() {
        for candidate in candidates("Müller & Söhne – Antiquitäten", 3) {
            assert_eq!(
                Ok(candidate.as_str()),
                ListingSourceSlugId::raw(&candidate)
                    .as_ref()
                    .map(AsRef::as_ref)
            );
        }
    }
}
