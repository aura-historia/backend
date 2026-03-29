#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShopPartnerStatus {
    #[default]
    Scraped,
    Partnered,
}
