use super::product_listing_content_assessment_reader::ProductListingContentAssessmentReadError;

use product_listing_core::{
    content_policy::ContentPolicyDecision, product_listing_id::ProductListingId,
};

/// Reads the assessment for a ProductListing's current content-source revision.
#[async_trait::async_trait]
pub trait ProductListingContentAssessmentSnapshotReader: Send {
    async fn find_current_for_product_listing(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ContentPolicyDecision>, ProductListingContentAssessmentReadError>;
}

pub trait ProductListingContentAssessmentSnapshotReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingContentAssessmentSnapshotReader + 'tx;
}
