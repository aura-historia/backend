use super::product_listing_content_assessment_reader::ProductListingContentAssessmentReadError;
use domain_primitives::event_id::EventId;
use product_listing_core::{
    content_policy::ContentPolicyDecision, product_listing_id::ProductListingId,
};

/// Reads the assessment exactly associated with a ProductListing source revision.
#[async_trait::async_trait]
pub trait ProductListingContentAssessmentSnapshotReader: Send {
    async fn find_for_source_event(
        &mut self,
        product_listing_id: ProductListingId,
        source_event_id: EventId,
    ) -> Result<Option<ContentPolicyDecision>, ProductListingContentAssessmentReadError>;
}

pub trait ProductListingContentAssessmentSnapshotReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingContentAssessmentSnapshotReader + 'tx;
}
