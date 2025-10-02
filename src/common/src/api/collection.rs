use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutCollectionData<T> {
    pub items: Vec<T>,
}
