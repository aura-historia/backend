#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShopPartnerStatus {
    #[default]
    Scraped,
    Partnered,
}
