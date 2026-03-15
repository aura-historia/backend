use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct DeleteResponse {
    #[serde(rename = "_index")]
    pub index: String,

    #[serde(rename = "_id")]
    pub id: String,

    #[serde(rename = "_version", default)]
    pub version: Option<u64>,

    pub result: String,
}
