use super::SchemaCandidateEvaluation;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SchemaMatrix {
    pub review_id: uuid::Uuid,
    pub candidates: Vec<SchemaCandidateEvaluation>,
}
