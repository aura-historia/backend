#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortCategoryField {
    #[default]
    Score,
    Name,
    Updated,
    Created,
}
