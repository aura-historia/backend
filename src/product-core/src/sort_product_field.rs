#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortProductField {
    #[default]
    Score,
    Price,
    Updated,
    Created,
}
