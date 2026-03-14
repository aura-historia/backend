#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub enum NotificationType {
    #[default]
    Email,
}
