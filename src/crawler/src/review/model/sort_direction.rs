//! The direction a console list sorts in.

use super::QueryValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDirection {
    Ascending,
    #[default]
    Descending,
}

impl SortDirection {
    /// The SQL keyword of the direction.
    pub const fn sql(self) -> &'static str {
        match self {
            Self::Ascending => "ASC",
            Self::Descending => "DESC",
        }
    }
}

impl QueryValue for SortDirection {
    const VALUES: &'static [(&'static str, Self)] =
        &[("asc", Self::Ascending), ("desc", Self::Descending)];
}
