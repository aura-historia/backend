#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum ShopType {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}
