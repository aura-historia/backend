use crate::page::Page;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaginatedResult<T, Key> {
    pub items: Vec<T>,
    pub page: Page<Key>,
    pub total: Option<u64>,
    pub next_after: Option<Key>,
}
