use application::{error::box_error, operation_context::OperationContext};
use listing_source_core::ListingSourceId;
use listing_source_postgres::SqlxSourceLocationAccessFactory;
use listing_source_service::{
    ports::location_default::{
        SourceLocationAccess, SourceLocationAccessFactory, SourceLocationAccessPolicy,
    },
    use_cases::location_default::{LocationDefaultError, SelectedLocationEligibility},
};
use party_core::party_id::PartyId;
use platform_postgres::SqlxTransaction;
use product_listing_core::{
    delivery::{DeliveryIntegration, ListingDelivery},
    product_listing_id::ProductListingKey,
};
use product_listing_service::{
    ports::{ProductListingStorageVersion, product_listing_delivery::*},
    use_cases::{ProductListingDeliveryError, ProductListingDeliveryView},
};
use shipping_core::{
    selection::{ComponentOverride, ProfileSelection},
    terms::{DeliverySite, DeliveryTerms, ShippingPolicy},
};
use sqlx::PgConnection;
#[derive(Clone, Copy, Default)]
pub struct SqlxProductListingDeliveryReferencesFactory;
#[derive(Clone, Copy, Default)]
pub struct SqlxProductListingDeliveryReaderFactory;
struct References<'a>(&'a mut SqlxTransaction);
struct Reader<'a>(&'a mut PgConnection);
impl ProductListingDeliveryReferencesFactory<SqlxTransaction>
    for SqlxProductListingDeliveryReferencesFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ProductListingDeliveryReferences + 'a {
        References(tx)
    }
}
impl ProductListingDeliveryReaderFactory<SqlxTransaction>
    for SqlxProductListingDeliveryReaderFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ProductListingDeliveryReader + 'a {
        Reader(tx.connection())
    }
}
fn fail(source: impl std::error::Error + Send + Sync + 'static) -> ProductListingDeliveryError {
    ProductListingDeliveryError::OperationFailed {
        source: box_error(source),
    }
}
fn access_error(e: LocationDefaultError) -> ProductListingDeliveryError {
    match e {
        LocationDefaultError::AuthenticationRequired => {
            ProductListingDeliveryError::AuthenticationRequired
        }
        LocationDefaultError::Forbidden => ProductListingDeliveryError::Forbidden,
        LocationDefaultError::SourceNotFound => ProductListingDeliveryError::SourceNotFound,
        LocationDefaultError::LocationNotFound | LocationDefaultError::LocationRetired => {
            ProductListingDeliveryError::ReferenceIneligible
        }
        e => fail(e),
    }
}
impl References<'_> {
    async fn sites(
        &mut self,
        owner: PartyId,
        terms: &DeliveryTerms,
    ) -> Result<(), ProductListingDeliveryError> {
        let origin = match terms.shipping() {
            Some(ShippingPolicy::Coverage(v)) => v.dispatch_origin(),
            _ => None,
        };
        for site in origin
            .into_iter()
            .chain(terms.collection().and_then(|v| v.site()))
        {
            if let DeliverySite::PartyLocation(id) = site
                && SqlxSourceLocationAccessFactory
                    .in_transaction(self.0)
                    .check_location(owner, *id)
                    .await
                    .map_err(access_error)?
                    != SelectedLocationEligibility::Active
            {
                return Err(ProductListingDeliveryError::ReferenceIneligible);
            }
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl ProductListingDeliveryReferences for References<'_> {
    async fn manual_takeover(
        &mut self,
        command: &product_listing_service::use_cases::ChangeProductListingDeliveryCommand,
        id: product_listing_core::product_listing_id::ProductListingId,
    ) -> Result<(), ProductListingDeliveryError> {
        use shipping_service::imports::{
            ShippingImportAccess, ShippingImportAccessFactory, ShippingImportSubject,
        };
        shipping_postgres::imports::SqlxShippingImportAccessFactory
            .in_transaction(self.0)
            .manual_takeover(
                command.product_key.listing_source_id,
                &command.import_operation(),
                ShippingImportSubject::Listing(id),
            )
            .await?;
        Ok(())
    }

    async fn prepare_import(
        &mut self,
        context: &OperationContext,
        command: &product_listing_service::use_cases::ChangeProductListingDeliveryCommand,
    ) -> Result<bool, ProductListingDeliveryError> {
        use shipping_service::imports::{ShippingImportAccess, ShippingImportAccessFactory};
        let write = command
            .import
            .as_ref()
            .ok_or(ProductListingDeliveryError::Unavailable)?;
        Ok(shipping_postgres::imports::SqlxShippingImportAccessFactory
            .in_transaction(self.0)
            .prepare(
                context,
                command.product_key.listing_source_id,
                write,
                &command.import_operation(),
            )
            .await?
            .replay)
    }
    async fn complete_import(
        &mut self,
        context: &OperationContext,
        command: &product_listing_service::use_cases::ChangeProductListingDeliveryCommand,
        id: product_listing_core::product_listing_id::ProductListingId,
    ) -> Result<(), ProductListingDeliveryError> {
        use shipping_service::imports::{
            ShippingImportAccess, ShippingImportAccessFactory, ShippingImportSubject,
        };
        let write = command
            .import
            .as_ref()
            .ok_or(ProductListingDeliveryError::Unavailable)?;
        shipping_postgres::imports::SqlxShippingImportAccessFactory
            .in_transaction(self.0)
            .complete(
                context,
                command.product_key.listing_source_id,
                write,
                &command.import_operation(),
                ShippingImportSubject::Listing(id),
            )
            .await?;
        Ok(())
    }
    async fn authorize(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
    ) -> Result<(), ProductListingDeliveryError> {
        SqlxSourceLocationAccessFactory
            .in_transaction(self.0)
            .authorize_source(
                &context.principal,
                source,
                SourceLocationAccessPolicy::AdministratorOrOperator,
            )
            .await
            .map_err(access_error)?;
        Ok(())
    }
    async fn lock_target(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<(), ProductListingDeliveryError> {
        sqlx::query_scalar::<_,uuid::Uuid>("SELECT product_listing_id FROM product_listings WHERE listing_source_id=$1 AND source_listing_id=$2 FOR UPDATE")
            .bind(key.listing_source_id.into_uuid()).bind(key.source_listing_id.as_ref()).fetch_optional(self.0.connection()).await.map_err(fail)?.ok_or(ProductListingDeliveryError::NotFound)?;
        Ok(())
    }
    async fn validate(&mut self, d: &ListingDelivery) -> Result<(), ProductListingDeliveryError> {
        let source = d.source_id();
        let owner: uuid::Uuid = sqlx::query_scalar(
            "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1",
        )
        .bind(source.into_uuid())
        .fetch_one(self.0.connection())
        .await
        .map_err(fail)?;
        let owner = PartyId::try_from(owner).map_err(fail)?;
        if let DeliveryIntegration::Provider { method } = d.integration() {
            let enabled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_source_ingestion_methods WHERE listing_source_id=$1 AND ingestion_method=$2)")
                .bind(source.into_uuid()).bind(method.as_str()).fetch_one(self.0.connection()).await.map_err(fail)?;
            if !enabled {
                return Err(ProductListingDeliveryError::IntegrationMismatch);
            }
        }
        let profile=match d.selection() {
            Some(ProfileSelection::Explicit(id))=>Some(id.into_uuid()),
            Some(ProfileSelection::InheritSourceDefault)=>{
                sqlx::query_scalar::<_,Option<uuid::Uuid>>("SELECT shipping_profile_id FROM shipping_source_defaults WHERE listing_source_id=$1 AND NOT source_deleted FOR SHARE")
                    .bind(source.into_uuid()).fetch_optional(self.0.connection()).await.map_err(fail)?.flatten()
            },_=>None,
        };
        let base = if let Some(id) = profile {
            let row:Option<(String,Option<String>,serde_json::Value)>=sqlx::query_as("SELECT lifecycle,integration_method,terms FROM shipping_profiles WHERE shipping_profile_id=$1 AND listing_source_id=$2 AND current_source_id=$2 AND (expires_at IS NULL OR expires_at>now()) AND NOT EXISTS(SELECT 1 FROM shipping_profile_import_state i WHERE i.shipping_profile_id=shipping_profiles.shipping_profile_id AND i.owner_hash IS NOT NULL AND NOT shipping_import_current(shipping_profiles.listing_source_id,i.source_revision)) FOR SHARE")
                .bind(id).bind(source.into_uuid()).fetch_optional(self.0.connection()).await.map_err(fail)?;
            let (lifecycle, method, terms) =
                row.ok_or(ProductListingDeliveryError::ReferenceIneligible)?;
            if lifecycle != "ACTIVE" {
                return Err(ProductListingDeliveryError::ReferenceIneligible);
            }
            let same = match d.integration() {
                DeliveryIntegration::Source => method.is_none(),
                DeliveryIntegration::Provider { method: m } => {
                    method.as_deref() == Some(m.as_str())
                }
            };
            if !same {
                return Err(ProductListingDeliveryError::IntegrationMismatch);
            }
            shipping_json::decode::<crate::delivery_codec::StoredDeliveryReferences>(terms, source)
                .map_err(|source| ProductListingDeliveryError::OperationFailed { source })?
        } else {
            match d.selection() {
                Some(ProfileSelection::Inline(t)) => t.as_ref().clone(),
                _ => DeliveryTerms::default(),
            }
        };
        // Coherent replacements discard prior geography. Cleared/replaced components do not require
        // the inherited location to remain eligible, so stale location assertions can be removed.
        let shipping = match &d.overrides().shipping {
            ComponentOverride::Inherit => base.shipping().cloned(),
            ComponentOverride::Clear => None,
            ComponentOverride::Replace(v) => Some(v.clone()),
        };
        let collection = match &d.overrides().collection {
            ComponentOverride::Inherit => base.collection().cloned(),
            ComponentOverride::Clear => None,
            ComponentOverride::Replace(v) => Some(v.clone()),
        };
        self.sites(owner, &DeliveryTerms::new(shipping, collection))
            .await
    }
}
#[async_trait::async_trait]
impl ProductListingDeliveryReader for Reader<'_> {
    async fn get(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<ProductListingDeliveryView, ProductListingDeliveryError> {
        let (id,version,delivery):(uuid::Uuid,i64,Option<serde_json::Value>)=sqlx::query_as("SELECT product_listing_id,version,delivery_configuration FROM product_listings WHERE listing_source_id=$1 AND source_listing_id=$2")
            .bind(key.listing_source_id.into_uuid()).bind(key.source_listing_id.as_ref()).fetch_optional(&mut *self.0).await.map_err(fail)?.ok_or(ProductListingDeliveryError::NotFound)?;
        let delivery = delivery
            .map(crate::delivery_codec::decode)
            .transpose()
            .map_err(|source| ProductListingDeliveryError::OperationFailed { source })?;
        if delivery
            .as_ref()
            .is_some_and(|d| d.source_id() != key.listing_source_id)
        {
            return Err(fail(std::io::Error::other(
                "invalid persisted delivery source",
            )));
        }
        Ok(ProductListingDeliveryView {
            product_listing_id:
                product_listing_core::product_listing_id::ProductListingId::try_from(id)
                    .map_err(fail)?,
            version: ProductListingStorageVersion::try_from(version).map_err(fail)?,
            delivery,
        })
    }
}
