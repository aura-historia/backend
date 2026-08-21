#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Localized<L, T> {
    pub localization: L,
    pub payload: T,
}

impl<L, T> Localized<L, T> {
    pub fn new(localization: L, payload: T) -> Self {
        Self {
            localization,
            payload,
        }
    }
}
