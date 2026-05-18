use super::SchemaCandidateEvaluation;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SchemaMatrix {
    pub review_id: uuid::Uuid,
    pub candidates: Vec<SchemaCandidateEvaluation>,
}
