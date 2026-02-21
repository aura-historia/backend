#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortPeriodField {
    #[default]
    Score,
    Name,
    Updated,
    Created,
}
