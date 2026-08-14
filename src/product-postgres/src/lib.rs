pub mod event_store;
pub mod fx_rate_snapshot_repository;
pub mod partner_product_authorizer;
pub mod product_embedding_source_reader;
pub mod product_embedding_writer;
pub mod product_translation_source_reader;
pub mod product_translation_writer;
pub mod readers;
pub mod repository;

pub use event_store::{SqlxProductEventStore, SqlxProductEventStoreFactory};
pub use fx_rate_snapshot_repository::SqlxFxRateSnapshotRepositoryFactory;
pub use partner_product_authorizer::SqlxPartnerProductAuthorizerFactory;
pub use product_embedding_source_reader::SqlxProductEmbeddingSourceReader;
pub use product_embedding_writer::SqlxProductEmbeddingWriterFactory;
pub use product_translation_source_reader::SqlxProductTranslationSourceReader;
pub use product_translation_writer::SqlxProductTranslationWriterFactory;
pub use readers::{
    SqlxProductDetailsBatchReader, SqlxProductDetailsReaderFactory,
    SqlxProductEmbeddingReaderFactory, SqlxProductEventReaderFactory,
    SqlxProductSearchFilterMatchSourceReaderFactory, SqlxProductUserStateReader,
    SqlxProductWatchlistDetailsReaderFactory, SqlxProductWatchlistNotificationSourceReaderFactory,
};
pub use repository::{SqlxProductRepository, SqlxProductRepositoryFactory};
