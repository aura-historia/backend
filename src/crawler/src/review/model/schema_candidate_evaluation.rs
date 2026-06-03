use super::SchemaPageEvaluation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaCandidateEvaluation {
    pub schema_index: usize,
    pub pages: Vec<SchemaPageEvaluation>,
}
