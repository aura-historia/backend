#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortShopField {
    #[default]
    Score,
    Name,
    Updated,
    Created,
}
