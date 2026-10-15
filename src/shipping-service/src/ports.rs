use crate::{
    ShippingError,
    use_cases::{DefaultView, ProfileSummary, ProfileView},
};
use listing_source_core::ListingSourceId;
use shipping_core::{
    profile::{ShippingProfile, ShippingProfileId},
    selection::SourceDefaultProfile,
};

domain_primitives::version_newtype!(ShippingManagementRevision);
domain_primitives::version_newtype!(ShippingDefaultRevision);

/// Protected provenance references only. Never fetch references or include buyer addresses.
#[derive(Clone, PartialEq, Eq)]
pub struct ShippingEvidence {
    reference: String,
    assertion: String,
}
impl ShippingEvidence {
    pub fn new(reference: String, assertion: String) -> Result<Self, ShippingError> {
        for (value, limit) in [(&reference, 2048), (&assertion, 255)] {
            if value.trim().is_empty()
                || value.len() > limit
                || value.chars().any(char::is_control)
                || value.contains(['\u{2028}', '\u{2029}'])
            {
                return Err(ShippingError::InvalidInput);
            }
        }
        Ok(Self {
            reference,
            assertion,
        })
    }
    pub fn reference(&self) -> &str {
        &self.reference
    }
    pub fn assertion(&self) -> &str {
        &self.assertion
    }
}
impl std::fmt::Debug for ShippingEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShippingEvidence(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredProfile {
    pub profile: ShippingProfile,
    pub management_revision: ShippingManagementRevision,
    pub evidence: Vec<ShippingEvidence>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct StoredDefault {
    pub selection: SourceDefaultProfile,
    pub revision: ShippingDefaultRevision,
}

#[async_trait::async_trait]
pub trait ShippingRepository: Send {
    fn imports(&mut self) -> impl crate::imports::ShippingImportAccess + '_;
    /// Immutable retry identity; source authorization must already be locked.
    async fn creation_retry(
        &mut self,
        source: ListingSourceId,
        caller: &str,
        key: &str,
        content: &shipping_core::profile::ShippingProfileContent,
        evidence: &[ShippingEvidence],
    ) -> Result<Option<StoredProfile>, ShippingError>;
    async fn insert(
        &mut self,
        profile: &StoredProfile,
        caller: &str,
        key: &str,
    ) -> Result<(), ShippingError>;
    async fn find_for_update(
        &mut self,
        source: ListingSourceId,
        id: ShippingProfileId,
    ) -> Result<StoredProfile, ShippingError>;
    /// Writes current state, protected history, evidence and minimal signal together.
    async fn save(
        &mut self,
        profile: &StoredProfile,
        expected: ShippingManagementRevision,
        actor: &str,
    ) -> Result<(), ShippingError>;
    async fn default_for_update(
        &mut self,
        source: ListingSourceId,
    ) -> Result<StoredDefault, ShippingError>;
    async fn save_default(
        &mut self,
        default: &StoredDefault,
        expected: ShippingDefaultRevision,
        actor: &str,
    ) -> Result<(), ShippingError>;
}
pub trait ShippingRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ShippingRepository + 'tx;
}

#[async_trait::async_trait]
pub trait ShippingReader: Send {
    async fn get(
        &mut self,
        source: ListingSourceId,
        id: ShippingProfileId,
    ) -> Result<ProfileView, ShippingError>;
    /// Keyset bounded summary read; never loads terms/evidence or dependent listings.
    async fn list(
        &mut self,
        source: ListingSourceId,
        after: Option<ShippingProfileId>,
        limit: u32,
    ) -> Result<Vec<ProfileSummary>, ShippingError>;
    async fn get_default(&mut self, source: ListingSourceId) -> Result<DefaultView, ShippingError>;
}
pub trait ShippingReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ShippingReader + 'tx;
}
