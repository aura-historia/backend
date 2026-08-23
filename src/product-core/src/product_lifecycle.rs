#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default)]
pub enum ProductLifecycle {
    #[default]
    Active,
    Deleted,
}
