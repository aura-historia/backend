//! Versioned queue contract and FIFO SQS publisher for product-listing ingestion.

pub mod codec;
pub mod publisher;

pub use publisher::{
    ScopedSqsProductListingIngestionPublisher, SqsProductListingIngestionPublisher,
    with_publication_deadline,
};
