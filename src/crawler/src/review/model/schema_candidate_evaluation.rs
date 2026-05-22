use super::SchemaPageEvaluation;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SchemaCandidateEvaluation {
    pub schema_index: usize,
    pub pages: Vec<SchemaPageEvaluation>,
}
