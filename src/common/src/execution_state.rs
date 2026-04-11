#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    Processing,
    Waiting,
    Completed,
}
