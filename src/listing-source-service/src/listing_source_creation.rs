use crate::ports::{
    ListingSourceIngestionConfigurations, ListingSourceRepository, ListingSourceRepositoryError,
    StoredListingSource,
};
use application::error::static_error;
use listing_source_core::{ListingSource, ListingSourceSlugId, NewListingSource};

/// Bounds the inserts one creation may spend on slug candidates of a heavily reused name.
const MAX_LISTING_SOURCE_SLUG_CANDIDATES: usize = 100;

/// Persists a new ListingSource with its ingestion configuration in the caller's transaction under
/// the first of its [`ListingSourceSlugId::candidates`] that no other ListingSource holds.
pub async fn insert_new_listing_source(
    sources: &mut impl ListingSourceRepository,
    input: NewListingSource,
    configuration: &ListingSourceIngestionConfigurations,
) -> Result<StoredListingSource, ListingSourceRepositoryError> {
    for slug_id in
        ListingSourceSlugId::candidates(&input.name).take(MAX_LISTING_SOURCE_SLUG_CANDIDATES)
    {
        match sources
            .insert(
                &ListingSource::create(input.clone(), slug_id),
                configuration,
            )
            .await
        {
            Err(ListingSourceRepositoryError::SlugConflict { .. }) => {}
            inserted => return inserted,
        }
    }
    Err(ListingSourceRepositoryError::SlugConflict {
        source: static_error("every tried listing source slug candidate is taken"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ListingIngestionConfiguration, ListingSourceDeletionBlocker, ListingSourceStorageVersion,
    };
    use listing_source_core::{
        ListingIngestionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
    };
    use party_core::party_id::PartyId;
    use std::collections::HashSet;
    use time::OffsetDateTime;

    #[derive(Default)]
    struct FakeSources {
        taken: HashSet<String>,
        insert_error: Option<ListingSourceRepositoryError>,
        attempted: Vec<String>,
    }

    fn unexpected() -> ListingSourceRepositoryError {
        ListingSourceRepositoryError::Internal {
            source: static_error("unexpected listing source repository call"),
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceRepository for FakeSources {
        async fn find_by_id(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Err(unexpected())
        }

        async fn find_by_slug(
            &mut self,
            _: &ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Err(unexpected())
        }

        async fn find_by_id_for_update(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Err(unexpected())
        }

        async fn find_deletion_blocker(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<ListingSourceDeletionBlocker>, ListingSourceRepositoryError> {
            Err(unexpected())
        }

        async fn delete_unused(
            &mut self,
            _: ListingSourceId,
            _: ListingSourceStorageVersion,
        ) -> Result<(), ListingSourceRepositoryError> {
            Err(unexpected())
        }

        async fn insert(
            &mut self,
            source: &ListingSource,
            configuration: &ListingSourceIngestionConfigurations,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            let slug_id = source.slug_id().to_string();
            self.attempted.push(slug_id.clone());
            if let Some(error) = self.insert_error.take() {
                return Err(error);
            }
            if !self.taken.insert(slug_id) {
                return Err(ListingSourceRepositoryError::SlugConflict {
                    source: static_error("listing source slug already exists"),
                });
            }
            Ok(StoredListingSource {
                source: source.clone(),
                configuration: configuration.clone(),
                version: ListingSourceStorageVersion::INITIAL,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            })
        }

        async fn update(
            &mut self,
            _: &ListingSource,
            _: &ListingSourceIngestionConfigurations,
            _: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(unexpected())
        }
    }

    fn new_source(name: &str) -> NewListingSource {
        NewListingSource {
            id: ListingSourceId::new(),
            name: ListingSourceName::try_from(name)
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            operator_party_id: PartyId::new(),
            ingestion_methods: HashSet::from([ListingIngestionMethod::PartnerApi]),
            presentation: ListingSourcePresentation::default(),
            platform_technology: None,
            referral_configuration: None,
        }
    }

    fn partner_api() -> ListingSourceIngestionConfigurations {
        ListingSourceIngestionConfigurations(vec![ListingIngestionConfiguration::PartnerApi])
    }

    #[tokio::test]
    async fn should_insert_listing_source_under_slugified_name_when_it_is_free() {
        let mut sources = FakeSources::default();

        let stored =
            insert_new_listing_source(&mut sources, new_source("Antik und Stil"), &partner_api())
                .await;

        assert!(matches!(
            stored,
            Ok(StoredListingSource { ref source, .. })
                if source.slug_id().as_ref() == "antik-und-stil"
        ));
        assert_eq!(vec!["antik-und-stil"], sources.attempted);
    }

    #[tokio::test]
    async fn should_insert_listing_source_under_first_free_numbered_slug_when_earlier_ones_are_taken()
     {
        let mut sources = FakeSources {
            taken: HashSet::from(["antik-und-stil".to_owned(), "antik-und-stil-2".to_owned()]),
            ..FakeSources::default()
        };

        let stored =
            insert_new_listing_source(&mut sources, new_source("Antik und Stil"), &partner_api())
                .await;

        assert!(matches!(
            stored,
            Ok(StoredListingSource { ref source, ref configuration, .. })
                if source.slug_id().as_ref() == "antik-und-stil-3"
                    && *configuration == partner_api()
        ));
        assert_eq!(
            vec!["antik-und-stil", "antik-und-stil-2", "antik-und-stil-3"],
            sources.attempted
        );
    }

    #[tokio::test]
    async fn should_report_slug_conflict_when_every_candidate_is_taken() {
        let input = new_source("Antik und Stil");
        let mut sources = FakeSources {
            taken: ListingSourceSlugId::candidates(&input.name)
                .take(MAX_LISTING_SOURCE_SLUG_CANDIDATES)
                .map(|slug_id| slug_id.to_string())
                .collect(),
            ..FakeSources::default()
        };

        let stored = insert_new_listing_source(&mut sources, input, &partner_api()).await;

        assert!(matches!(
            stored,
            Err(ListingSourceRepositoryError::SlugConflict { .. })
        ));
        assert_eq!(MAX_LISTING_SOURCE_SLUG_CANDIDATES, sources.attempted.len());
    }

    #[tokio::test]
    async fn should_not_try_further_slugs_after_other_insert_failure() {
        let mut sources = FakeSources {
            insert_error: Some(ListingSourceRepositoryError::ShopifyDomainConflict {
                source: static_error("shop domain already configured"),
            }),
            ..FakeSources::default()
        };

        let stored =
            insert_new_listing_source(&mut sources, new_source("Antik und Stil"), &partner_api())
                .await;

        assert!(matches!(
            stored,
            Err(ListingSourceRepositoryError::ShopifyDomainConflict { .. })
        ));
        assert_eq!(vec!["antik-und-stil"], sources.attempted);
    }
}
