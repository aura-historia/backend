use super::SchemaCandidateEvaluation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaMatrix {
    pub review_id: uuid::Uuid,
    pub candidates: Vec<SchemaCandidateEvaluation>,
}
