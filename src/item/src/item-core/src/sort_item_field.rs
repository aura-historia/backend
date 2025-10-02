#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortItemField {
    #[default]
    Score,
    Price,
    Updated,
    Created,
}
