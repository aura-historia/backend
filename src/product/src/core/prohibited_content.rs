#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
pub enum ProhibitedContent {
    #[default]
    Unknown,
    None,
    NaziGermany,
}
