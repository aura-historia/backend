use application::error::BoxError;
use listing_source_core::{
    AcquisitionMethod, ListingSourceId, ListingSourceName, ListingSourceSlugId,
};
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ListingSourceDetails {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
    pub operator_party_id: PartyId,
    pub operator_slug_id: PartySlugId,
    pub operator_name: PartyName,
    pub acquisition_methods: std::collections::HashSet<AcquisitionMethod>,
    pub url: Option<url::Url>,
    pub image: Option<url::Url>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceReadError {
    #[error("temporary listing source read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid listing source read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListingSourceDetailsReader: Send + Sync {
    async fn find_details_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError>;
    async fn find_details_by_slug(
        &self,
        slug: &ListingSourceSlugId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError>;
}
