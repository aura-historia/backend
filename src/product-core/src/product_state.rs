#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, strum_macros::EnumCount)]
pub enum ProductState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}
