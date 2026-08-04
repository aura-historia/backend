pub mod event_store;
pub mod readers;
pub mod repository;

pub use event_store::{SqlxProductEventStore, SqlxProductEventStoreFactory};
pub use readers::{
    SqlxProductDetailsReaderFactory, SqlxProductEmbeddingReaderFactory,
    SqlxProductEventReaderFactory, SqlxProductWatchlistDetailsReaderFactory,
};
pub use repository::{SqlxProductRepository, SqlxProductRepositoryFactory};
