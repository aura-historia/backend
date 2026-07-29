#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShopPartnerStatus {
    #[default]
    Scraped,
    Partnered,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_default_to_scraped_when_status_not_set() {
        assert_eq!(ShopPartnerStatus::Scraped, ShopPartnerStatus::default());
    }

    #[test]
    fn should_distinguish_partner_status_variants_when_hashed() {
        let statuses = HashSet::from([ShopPartnerStatus::Scraped, ShopPartnerStatus::Partnered]);

        assert_eq!(2, statuses.len());
        assert!(statuses.contains(&ShopPartnerStatus::Scraped));
        assert!(statuses.contains(&ShopPartnerStatus::Partnered));
    }
}
