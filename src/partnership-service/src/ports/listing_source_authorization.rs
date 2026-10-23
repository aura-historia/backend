use application::error::BoxError;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct AdministeredListingSource {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdministeredListingSourcePosition {
    pub name: ListingSourceName,
    pub listing_source_id: ListingSourceId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct AdministeredListingSourcePage {
    pub items: Vec<AdministeredListingSource>,
    pub next: Option<AdministeredListingSourcePosition>,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceAuthorizationError {
    #[error("temporary listing source authorization failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid listing source authorization data")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source authorization failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListingSourceAuthorization: Send + Sync {
    async fn can_write_source(
        &self,
        user_id: UserId,
        listing_source_id: ListingSourceId,
    ) -> Result<bool, SourceAuthorizationError>;
    async fn list_sources_user_administers(
        &self,
        user_id: UserId,
        size: u32,
        after: Option<&AdministeredListingSourcePosition>,
    ) -> Result<AdministeredListingSourcePage, SourceAuthorizationError>;
}
