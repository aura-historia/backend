use crate::page::Page;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaginatedResult<T, Key> {
    pub items: Vec<T>,
    pub page: Page<Key>,
    pub total: Option<u64>,
    pub next_after: Option<Key>,
}

impl<T, Key> PaginatedResult<T, Key> {
    pub fn map_item<U, F>(self, f: F) -> PaginatedResult<U, Key>
    where
        F: FnMut(T) -> U,
    {
        PaginatedResult {
            items: self.items.into_iter().map(f).collect(),
            page: self.page,
            total: self.total,
            next_after: self.next_after,
        }
    }
}
