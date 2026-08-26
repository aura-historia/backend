use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "OpenSearch request timed out during {request_kind} after {took} ms (failed_shards={failed_shards}/{total_shards})"
)]
pub struct OpenSearchTimedOutError {
    pub request_kind: &'static str,
    pub took: u64,
    pub total_shards: u64,
    pub successful_shards: u64,
    pub skipped_shards: u64,
    pub failed_shards: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchResponse<T> {
    pub took: u64,
    pub timed_out: bool,
    #[serde(rename = "_shards")]
    pub shards: ShardStats,
    pub hits: HitsMetadata<T>,
}

impl<T> SearchResponse<T> {
    pub fn into_non_timed_out(
        self,
        request_kind: &'static str,
    ) -> Result<Self, OpenSearchTimedOutError> {
        if self.timed_out {
            Err(OpenSearchTimedOutError {
                request_kind,
                took: self.took,
                total_shards: self.shards.total,
                successful_shards: self.shards.successful,
                skipped_shards: self.shards.skipped,
                failed_shards: self.shards.failed,
            })
        } else {
            Ok(self)
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct ShardStats {
    pub total: u64,
    pub successful: u64,
    pub skipped: u64,
    pub failed: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HitsMetadata<T> {
    pub total: TotalHits,
    pub max_score: Option<f64>,
    pub hits: Vec<SearchHit<T>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TotalHits {
    pub value: u64,
    pub relation: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchHit<T> {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_score")]
    pub score: Option<f64>,
    #[serde(default)]
    pub sort: Option<serde_json::Value>,
    #[serde(default)]
    pub matched_queries: Vec<String>,
    #[serde(rename = "_source")]
    pub source: T,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Document {
        name: String,
    }

    #[test]
    fn should_deserialize_search_response_wire_fields() {
        let payload = r#"{
            "took": 10,
            "timed_out": false,
            "_shards": {"total": 2, "successful": 2, "skipped": 0, "failed": 0},
            "hits": {
                "total": {"value": 1, "relation": "eq"},
                "max_score": 1.0,
                "hits": [{
                    "_index": "product-listings",
                    "_id": "product-1",
                    "_score": 1.0,
                    "sort": ["product-1"],
                    "matched_queries": ["active"],
                    "_source": {"name": "ProductListing"}
                }]
            }
        }"#;

        let response: SearchResponse<Document> = serde_json::from_str(payload)
            .unwrap_or_else(|error| panic!("response must deserialize: {error}"));

        assert_eq!(response.took, 10);
        assert_eq!(response.shards.successful, 2);
        assert_eq!(response.hits.total.value, 1);
        assert_eq!(response.hits.hits[0].id, "product-1");
        assert_eq!(response.hits.hits[0].source.name, "ProductListing");
        assert_eq!(response.hits.hits[0].matched_queries, ["active"]);
    }

    #[test]
    fn should_return_timeout_details_when_response_timed_out() {
        let response = SearchResponse::<Document> {
            took: 321,
            timed_out: true,
            shards: ShardStats {
                total: 4,
                successful: 3,
                skipped: 0,
                failed: 1,
            },
            hits: HitsMetadata {
                total: TotalHits {
                    value: 0,
                    relation: "eq".to_owned(),
                },
                max_score: None,
                hits: vec![],
            },
        };

        let error = match response.into_non_timed_out("product search") {
            Err(error) => error,
            Ok(_) => panic!("timed-out response must return an error"),
        };

        assert_eq!(error.request_kind, "product search");
        assert_eq!(error.took, 321);
        assert_eq!(error.total_shards, 4);
        assert_eq!(error.successful_shards, 3);
        assert_eq!(error.failed_shards, 1);
    }
}
