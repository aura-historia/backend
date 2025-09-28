use crate::paginated_result::PaginatedResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetCollectionData<T, Key> {
    pub items: Vec<T>,
    pub pagination: PaginationData<Key>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginationData<Key> {
    pub from: Key,
    pub size: u64,
    pub total: Option<u64>,
    pub next: Option<Key>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutCollectionData<T> {
    pub items: Vec<T>,
}

impl<T, Key> From<PaginatedResult<T, Key>> for GetCollectionData<T, Key> {
    fn from(paginated: PaginatedResult<T, Key>) -> Self {
        GetCollectionData {
            items: paginated.items,
            pagination: PaginationData {
                from: paginated.page.from,
                size: paginated.page.size as u64,
                total: paginated.total,
                next: paginated.next_after,
            },
        }
    }
}
