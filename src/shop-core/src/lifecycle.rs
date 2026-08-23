#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShopLifecycle {
    #[default]
    Drafted,
    Published,
    Discarded,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_to_drafted() {
        assert_eq!(ShopLifecycle::Drafted, ShopLifecycle::default());
    }
}
