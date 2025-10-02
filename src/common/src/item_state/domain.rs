use strum_macros::EnumCount;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, EnumCount)]
pub enum ItemState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}
