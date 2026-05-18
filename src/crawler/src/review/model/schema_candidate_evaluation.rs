use super::SchemaPageEvaluation;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SchemaCandidateEvaluation {
    pub schema_index: usize,
    pub pages: Vec<SchemaPageEvaluation>,
}
