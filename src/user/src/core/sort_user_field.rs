#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortUserField {
    #[default]
    Score,
    Email,
    FirstName,
    LastName,
    Tier,
    Role,
    Created,
    Updated,
}
