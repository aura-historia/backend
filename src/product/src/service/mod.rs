#[cfg(all(feature = "dynamodb", feature = "sqs"))]
pub mod upsert_service;

#[cfg(feature = "dynamodb")]
pub mod enrichment_service;

#[cfg(feature = "dynamodb")]
pub mod get_service;
pub mod product_command;

#[cfg(all(feature = "watchlist", feature = "dynamodb"))]
pub mod personalization_service;

#[cfg(feature = "opensearch")]
pub mod query_service;

#[cfg(all(feature = "opensearch", feature = "dynamodb"))]
pub mod semantic_service;
