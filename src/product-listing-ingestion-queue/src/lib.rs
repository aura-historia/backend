//! Versioned wire contract and PostgreSQL job queue admission for ProductListing ingestion.

pub mod codec;
pub mod delivery;
pub mod publisher;

pub use publisher::{
    AdmissionError, IngestionAdmission, IngestionJob, PostgresIngestionAdmission,
    QueueProductListingIngestionPublisher, ScopedProductListingIngestionPublisher, rejection_code,
    with_publication_deadline,
};
