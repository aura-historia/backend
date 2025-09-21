use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetCollectionData<T> {
    pub items: Vec<T>,
    pub pagination: PaginationData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginationData {
    pub from: u64,
    pub size: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutCollectionData<T> {
    pub items: Vec<T>,
}
