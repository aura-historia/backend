use crate::percolation_document::{ProductPercolationDocumentError, product_document};
use common::error::boxed::box_error;
use fxrate_core::FxRateSnapshot;
use opensearch::{DeleteParts, IndexParts, OpenSearch, http::StatusCode, params::VersionType};
use product_core::product_id::ProductId;
use product_service::ports::{
    ProductSearchFilterMatchSource, ProductSearchProjection, ProductSearchProjectionWriteError,
    ProductSearchProjectionWriteOutcome,
};

const DEFAULT_INDEX: &str = "products";

#[derive(Clone)]
pub struct OpenSearchProductSearchProjection {
    client: OpenSearch,
    index: String,
}

impl OpenSearchProductSearchProjection {
    pub fn new(client: OpenSearch) -> Self {
        Self {
            client,
            index: DEFAULT_INDEX.to_owned(),
        }
    }

    pub fn with_index(client: OpenSearch, index: impl Into<String>) -> Self {
        Self {
            client,
            index: index.into(),
        }
    }
}

#[async_trait::async_trait]
impl ProductSearchProjection for OpenSearchProductSearchProjection {
    async fn upsert(
        &self,
        source: &ProductSearchFilterMatchSource,
        sale_snapshot: Option<&FxRateSnapshot>,
    ) -> Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError> {
        let version = checked_source_version(source.projection_version)?;
        let document = product_document(source, sale_snapshot).map_err(document_error)?;
        let response = self
            .client
            .index(IndexParts::IndexId(
                &self.index,
                &source.product_id.to_string(),
            ))
            .version(version)
            .version_type(VersionType::External)
            .body(document)
            .send()
            .await
            .map_err(|source| ProductSearchProjectionWriteError::WriteFailed {
                source: box_error(source),
            })?;
        write_outcome(response.status_code(), true)
    }

    async fn delete(
        &self,
        product_id: ProductId,
        source_version: i64,
    ) -> Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError> {
        let version = checked_source_version(source_version)?;
        let response = self
            .client
            .delete(DeleteParts::IndexId(&self.index, &product_id.to_string()))
            .version(version)
            .version_type(VersionType::External)
            .send()
            .await
            .map_err(|source| ProductSearchProjectionWriteError::DeleteFailed {
                source: box_error(source),
            })?;
        write_outcome(response.status_code(), false)
    }
}

fn checked_source_version(version: i64) -> Result<i64, ProductSearchProjectionWriteError> {
    (version >= 1).then_some(version).ok_or_else(|| {
        ProductSearchProjectionWriteError::WriteFailed {
            source: common::error::boxed::static_error(
                "Product projection version must be positive",
            ),
        }
    })
}

fn document_error(source: ProductPercolationDocumentError) -> ProductSearchProjectionWriteError {
    ProductSearchProjectionWriteError::WriteFailed {
        source: box_error(source),
    }
}

fn write_outcome(
    status: StatusCode,
    is_write: bool,
) -> Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError> {
    if status == StatusCode::CONFLICT {
        return Ok(ProductSearchProjectionWriteOutcome::Stale);
    }
    if status.is_success() {
        return Ok(ProductSearchProjectionWriteOutcome::Applied);
    }
    let error = common::error::boxed::static_error(
        "OpenSearch Product projection returned an unsuccessful status",
    );
    Err(if is_write {
        ProductSearchProjectionWriteError::WriteFailed { source: error }
    } else {
        ProductSearchProjectionWriteError::DeleteFailed { source: error }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_non_positive_projection_version() {
        assert!(matches!(
            checked_source_version(0),
            Err(ProductSearchProjectionWriteError::WriteFailed { .. })
        ));
    }

    #[test]
    fn should_accept_positive_projection_version() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(42, checked_source_version(42)?);
        Ok(())
    }

    #[test]
    fn should_map_conflict_to_stale_for_writes_and_deletes()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProductSearchProjectionWriteOutcome::Stale,
            write_outcome(StatusCode::CONFLICT, true)?
        );
        assert_eq!(
            ProductSearchProjectionWriteOutcome::Stale,
            write_outcome(StatusCode::CONFLICT, false)?
        );
        Ok(())
    }

    #[test]
    fn should_map_success_to_applied_for_writes_and_deletes()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProductSearchProjectionWriteOutcome::Applied,
            write_outcome(StatusCode::CREATED, true)?
        );
        assert_eq!(
            ProductSearchProjectionWriteOutcome::Applied,
            write_outcome(StatusCode::OK, false)?
        );
        Ok(())
    }

    #[test]
    fn should_map_unsuccessful_status_to_operation_specific_error() {
        assert!(matches!(
            write_outcome(StatusCode::INTERNAL_SERVER_ERROR, true),
            Err(ProductSearchProjectionWriteError::WriteFailed { .. })
        ));
        assert!(matches!(
            write_outcome(StatusCode::BAD_GATEWAY, false),
            Err(ProductSearchProjectionWriteError::DeleteFailed { .. })
        ));
    }
}
