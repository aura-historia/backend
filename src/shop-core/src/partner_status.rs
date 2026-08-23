#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, strum_macros::EnumIter)]
pub enum ShopPartnerStatus {
    #[default]
    Scraped,
    Partnered,
}

impl ShopPartnerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scraped => "SCRAPED",
            Self::Partnered => "PARTNERED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_default_to_scraped_when_status_not_set() {
        assert_eq!(ShopPartnerStatus::Scraped, ShopPartnerStatus::default());
    }

    #[test]
    fn should_use_unique_canonical_partner_status_identifiers() {
        let statuses = ShopPartnerStatus::iter()
            .map(ShopPartnerStatus::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(ShopPartnerStatus::iter().count(), statuses.len());
        assert_eq!("SCRAPED", ShopPartnerStatus::Scraped.as_str());
        assert_eq!("PARTNERED", ShopPartnerStatus::Partnered.as_str());
    }
}
