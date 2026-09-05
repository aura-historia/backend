use application::error::BoxError;
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use product_listing_normalization::{
    NormalizationInputHash, ProductListingNormalizationInput, RawProductListingProvenance,
};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use time::OffsetDateTime;
use uuid::Uuid;

const SHA256_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductListingRawStreamId(Uuid);

impl ProductListingRawStreamId {
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl From<ProductListingRawStreamId> for Uuid {
    fn from(value: ProductListingRawStreamId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductListingRawRevisionId(Uuid);

impl ProductListingRawRevisionId {
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl From<ProductListingRawRevisionId> for Uuid {
    fn from(value: ProductListingRawRevisionId) -> Self {
        value.0
    }
}

/// Raw ingestion methods intentionally exclude `PARTNER_API`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum ProductListingRawIngestionMethod {
    WebCrawl,
    Shopify,
    Woocommerce,
}

impl ProductListingRawIngestionMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebCrawl => "WEB_CRAWL",
            Self::Shopify => "SHOPIFY",
            Self::Woocommerce => "WOOCOMMERCE",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|method| method.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRecordKeySha256([u8; SHA256_BYTES]);

impl SourceRecordKeySha256 {
    pub const fn new(value: [u8; SHA256_BYTES]) -> Self {
        Self(value)
    }

    pub const fn as_bytes(&self) -> &[u8; SHA256_BYTES] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingRawCaptureWrite {
    pub listing_source_id: ListingSourceId,
    pub ingestion_method: ProductListingRawIngestionMethod,
    pub source_record_key: String,
    pub source_record_key_sha256: SourceRecordKeySha256,
    pub input: ProductListingNormalizationInput,
    pub input_sha256: NormalizationInputHash,
    pub provenance: RawProductListingProvenance,
    pub source_event_id: Option<String>,
    pub source_occurred_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductListingRawCaptureWriteOutcome {
    Changed {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        revision: u64,
    },
    Unchanged {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        latest_revision: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRawCaptureWriteError {
    #[error("source record key hash collision")]
    SourceRecordKeyHashCollision,
    #[error("raw product listing capture failed")]
    CaptureFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait]
pub trait ProductListingRawCaptureWriter: Send {
    async fn capture(
        &mut self,
        write: ProductListingRawCaptureWrite,
    ) -> Result<ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriteError>;
}

pub trait ProductListingRawCaptureWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx)
    -> impl ProductListingRawCaptureWriter + 'tx;
}
