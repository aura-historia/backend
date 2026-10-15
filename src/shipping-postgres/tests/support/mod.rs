use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{delivery::ListingDelivery, product_listing_id::ProductListingId};
use shipping_core::{
    fixtures::gallery_terms,
    profile::ShippingProfileId,
    selection::{ComponentOverride, DeliveryOverrides, ProfileSelection},
    terms::ShippingPolicy,
};
use shipping_postgres::assessment::SqlxAssessmentReaderFactory;
use shipping_service::assessment::{
    AssessmentDependencies, AssessmentReader, AssessmentReaderFactory,
};
use sqlx::PgPool;
use test_api::Postgres;
#[allow(dead_code)]
pub const SCHEMA: Postgres = Postgres::new("migrations");
pub struct References;
impl shipping_json::ShippingTermsReferenceCodec for References {
    fn decode_party(v: &str) -> Result<PartyId, application::error::BoxError> {
        Ok(PartyId::try_from(v.parse::<uuid::Uuid>()?)?)
    }
    fn encode_party(v: PartyId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_location(
        v: &str,
    ) -> Result<party_core::party_location_id::PartyLocationId, application::error::BoxError> {
        Ok(party_core::party_location_id::PartyLocationId::try_from(
            v.parse::<uuid::Uuid>()?,
        )?)
    }
    fn encode_location(v: party_core::party_location_id::PartyLocationId) -> String {
        v.into_uuid().to_string()
    }
}
impl shipping_json::listing_delivery::DeliveryReferenceCodec for References {
    fn decode_source(v: &str) -> Result<ListingSourceId, application::error::BoxError> {
        Ok(ListingSourceId::try_from(v.parse::<uuid::Uuid>()?)?)
    }
    fn encode_source(v: ListingSourceId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_profile(v: &str) -> Result<ShippingProfileId, application::error::BoxError> {
        Ok(ShippingProfileId::try_from(v.parse::<uuid::Uuid>()?)?)
    }
    fn encode_profile(v: ShippingProfileId) -> String {
        v.into_uuid().to_string()
    }
}
pub async fn seed(p: &PgPool) -> (ListingSourceId, ShippingProfileId, Vec<ProductListingId>) {
    let source = ListingSourceId::new();
    let owner = PartyId::new();
    let profile = ShippingProfileId::new();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name)VALUES($1,$2,'Gallery')")
        .bind(owner.into_uuid())
        .bind(format!("gallery-{}", owner.as_uuid().simple()))
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id)VALUES($1,$2,'Gallery',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid().simple())).bind(owner.into_uuid()).execute(p).await.unwrap();
    sqlx::query("INSERT INTO shipping_profiles(shipping_profile_id,listing_source_id,current_source_id,management_revision,terms_revision,lifecycle,name,terms)VALUES($1,$2,$2,1,1,'ACTIVE','Gallery',$3)").bind(profile.into_uuid()).bind(source.into_uuid()).bind(shipping_json::encode::<References>(&gallery_terms(8000)).unwrap()).execute(p).await.unwrap();
    sqlx::query("INSERT INTO shipping_source_defaults(listing_source_id,current_source_id,shipping_profile_id,revision)VALUES($1,$1,$2,2)").bind(source.into_uuid()).bind(profile.into_uuid()).execute(p).await.unwrap();
    let mut ids = Vec::new();
    for cabinet in [false, true] {
        let id = ProductListingId::new();
        let event = uuid::Uuid::now_v7();
        let delivery = if cabinet {
            Some(
                shipping_json::listing_delivery::encode::<References>(
                    &ListingDelivery::new(
                        source,
                        Default::default(),
                        Some(ProfileSelection::InheritSourceDefault),
                        Default::default(),
                        DeliveryOverrides {
                            shipping: ComponentOverride::Replace(ShippingPolicy::Prohibited),
                            collection: Default::default(),
                        },
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
        } else {
            None
        };
        let mut listing_tx = p.begin().await.unwrap();
        sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,lifecycle,url,delivery_configuration)VALUES($1,$2,$3,$3,$3,$4,$2,'ACTIVE','https://example.test/item',$5)").bind(id.into_uuid()).bind(format!("item-{}-abcdef",id.as_uuid().simple())).bind(event).bind(source.into_uuid()).bind(delivery).execute(&mut *listing_tx).await.unwrap();
        sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time)VALUES($1,$2,'PRODUCT_LISTING_DISCOVERED','DOMAIN',1,$3,now())").bind(event).bind(id.into_uuid()).bind(serde_json::json!({"listingSourceId":source.into_uuid().to_string(),"sourceListingId":format!("item-{}-abcdef",id.as_uuid().simple()),"title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/item","imageCount":0,"auction":null})).execute(&mut *listing_tx).await.unwrap();
        listing_tx.commit().await.unwrap();
        ids.push(id);
    }
    (source, profile, ids)
}
#[allow(dead_code)]
pub async fn read(p: &PgPool, ids: &[ProductListingId]) -> AssessmentDependencies {
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(ids, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    deps
}
