#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShopPartnerStatus {
    #[default]
    Scraped,
    Partnered,
}
