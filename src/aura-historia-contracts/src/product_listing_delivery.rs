//! Public delivery commands, results and pure codecs shared by transports and ingestion.
pub use crate::shipping::terms::PublicReferences as PublicDeliveryReferences;
use application::{
    error::{BoxError, box_error, static_error},
    patch_field::PatchField,
};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_service::{
    ports::ProductListingStorageVersion,
    use_cases::{ChangeProductListingDeliveryCommand, ProductListingDeliveryPatch},
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shipping_core::profile::ShippingProfileId;
use shipping_json::listing_delivery::{self, DeliveryReferenceCodec};
impl DeliveryReferenceCodec for PublicDeliveryReferences {
    fn decode_source(v: &str) -> Result<ListingSourceId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_source(v: ListingSourceId) -> String {
        v.to_string()
    }
    fn decode_profile(v: &str) -> Result<ShippingProfileId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_profile(v: ShippingProfileId) -> String {
        v.to_string()
    }
}
pub use crate::shipping::imports::ImportData;
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandData {
    #[schemars(range(min = 1, max = i64::MAX))]
    expected_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import: Option<ImportData>,
    delivery: WirePatch<PatchData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchData {
    #[schemars(schema_with = "listing_delivery::integration_schema")]
    integration: Value,
    #[schemars(schema_with = "selection_patch_schema")]
    selection: WirePatch<Value>,
    #[schemars(schema_with = "inputs_patch_schema")]
    rate_inputs: WirePatch<Value>,
    #[schemars(schema_with = "shipping_patch_schema")]
    shipping: WirePatch<Value>,
    #[schemars(schema_with = "collection_patch_schema")]
    collection: WirePatch<Value>,
}
pub fn decode_command(
    value: Value,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, BoxError> {
    let data: CommandData = serde_json::from_value(value).map_err(box_error)?;
    decode_data(data, key)
}
/// Parse typed command fields directly, preserving duplicate-field rejection at HTTP ingress.
pub fn decode_command_text(
    body: &str,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, BoxError> {
    let data: CommandData = serde_json::from_str(body).map_err(box_error)?;
    decode_data(data, key)
}
fn decode_data(
    data: CommandData,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, BoxError> {
    if data.expected_version > i64::MAX as u64 {
        return Err(static_error("invalid delivery version"));
    }
    let source = key.listing_source_id;
    let delivery = map_patch(data.delivery, |p| {
        Ok(ProductListingDeliveryPatch {
            integration: listing_delivery::decode_integration(p.integration)?,
            selection: map_patch(p.selection, |v| {
                listing_delivery::decode_selection::<PublicDeliveryReferences>(v, source)
            })?,
            rate_inputs: map_patch(p.rate_inputs, |v| {
                listing_delivery::decode_inputs(v, source)
            })?,
            shipping: map_patch(p.shipping, |v| {
                listing_delivery::decode_shipping::<PublicDeliveryReferences>(v, source)
            })?,
            collection: map_patch(p.collection, |v| {
                listing_delivery::decode_collection::<PublicDeliveryReferences>(v, source)
            })?,
        })
    })?;
    let command = ChangeProductListingDeliveryCommand {
        import: data.import.map(ImportData::into_command).transpose()?,
        product_key: key,
        expected_version: ProductListingStorageVersion::try_from(data.expected_version)
            .map_err(box_error)?,
        delivery,
    };
    if let PatchField::Set(p) = &command.delivery {
        p.apply(source, None).map_err(box_error)?;
    }
    Ok(command)
}
fn patch<T>(
    p: &PatchField<T>,
    f: impl FnOnce(&T) -> Result<Value, BoxError>,
) -> Result<WirePatch<Value>, BoxError> {
    Ok(match p {
        PatchField::Unchanged => WirePatch::Unchanged,
        PatchField::Clear => WirePatch::Clear,
        PatchField::Set(v) => WirePatch::Set(f(v)?),
    })
}
pub fn encode_command(c: &ChangeProductListingDeliveryCommand) -> Result<Value, BoxError> {
    let delivery = match &c.delivery {
        PatchField::Unchanged => WirePatch::Unchanged,
        PatchField::Clear => WirePatch::Clear,
        PatchField::Set(p) => WirePatch::Set(PatchData {
            integration: listing_delivery::encode_integration(&p.integration)?,
            selection: patch(
                &p.selection,
                listing_delivery::encode_selection::<PublicDeliveryReferences>,
            )?,
            rate_inputs: patch(&p.rate_inputs, listing_delivery::encode_inputs)?,
            shipping: patch(
                &p.shipping,
                listing_delivery::encode_shipping::<PublicDeliveryReferences>,
            )?,
            collection: patch(
                &p.collection,
                listing_delivery::encode_collection::<PublicDeliveryReferences>,
            )?,
        }),
    };
    serde_json::to_value(CommandData {
        expected_version: c.expected_version.into_inner(),
        import: c
            .import
            .as_ref()
            .map(ImportData::from_command)
            .transpose()?,
        delivery,
    })
    .map_err(box_error)
}

/// Explicit tagged patches shared by public delivery commands and durable ingestion codecs.
/// Omission and null semantics for ordinary REST patches have a separate contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum WirePatch<T> {
    #[schemars(transform = listing_delivery::allow_null_unit_payload)]
    Unchanged,
    #[schemars(transform = listing_delivery::allow_null_unit_payload)]
    Clear,
    Set(T),
}
impl<T> WirePatch<T> {
    pub fn try_map<U, E>(self, map: impl FnOnce(T) -> Result<U, E>) -> Result<PatchField<U>, E> {
        Ok(match self {
            Self::Unchanged => PatchField::Unchanged,
            Self::Clear => PatchField::Clear,
            Self::Set(value) => PatchField::Set(map(value)?),
        })
    }
}
fn map_patch<T, U>(
    patch: WirePatch<T>,
    map: impl FnOnce(T) -> Result<U, BoxError>,
) -> Result<PatchField<U>, BoxError> {
    patch.try_map(map)
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryData {
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: product_listing_core::product_listing_id::ProductListingId,
    #[schemars(range(min = 1, max = i64::MAX))]
    expected_version: u64,
    #[schemars(schema_with = "optional_delivery_schema")]
    delivery: Option<Value>,
}
impl TryFrom<product_listing_service::use_cases::ProductListingDeliveryView> for DeliveryData {
    type Error = BoxError;
    fn try_from(
        view: product_listing_service::use_cases::ProductListingDeliveryView,
    ) -> Result<Self, BoxError> {
        Ok(Self {
            product_listing_id: view.product_listing_id,
            expected_version: view.version.into_inner(),
            delivery: view
                .delivery
                .as_ref()
                .map(shipping_json::listing_delivery::encode::<PublicDeliveryReferences>)
                .transpose()?,
        })
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MutationData {
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: product_listing_core::product_listing_id::ProductListingId,
    #[schemars(range(min = 1, max = i64::MAX))]
    expected_version: u64,
    #[schemars(schema_with = "outcome_schema")]
    outcome: &'static str,
}
impl From<product_listing_service::use_cases::ChangeProductListingDeliveryResult> for MutationData {
    fn from(
        result: product_listing_service::use_cases::ChangeProductListingDeliveryResult,
    ) -> Self {
        Self {
            product_listing_id: result.product_listing_id,
            expected_version: result.version.into_inner(),
            outcome: if result.outcome.changed() {
                "CHANGED"
            } else {
                "UNCHANGED"
            },
        }
    }
}

fn references() -> (Schema, Schema) {
    (
        crate::wire::schema::object_id(PartyId::PREFIX),
        crate::wire::schema::object_id(PartyLocationId::PREFIX),
    )
}
fn patch_schema(generator: &mut SchemaGenerator, value: Schema) -> Schema {
    let mut schema = WirePatch::<Value>::json_schema(generator);
    if let Some(variants) = schema.get_mut("oneOf").and_then(Value::as_array_mut) {
        for variant in variants {
            if variant
                .pointer("/properties/state/const")
                .and_then(Value::as_str)
                == Some("SET")
                && let Some(target) = variant.pointer_mut("/properties/value")
            {
                *target = value.as_value().clone();
            }
        }
    }
    schema
}
fn selection_patch_schema(generator: &mut SchemaGenerator) -> Schema {
    let terms = crate::shipping::terms::schema(generator);
    let schema = listing_delivery::selection_schema(
        generator,
        crate::wire::schema::object_id(ShippingProfileId::PREFIX),
        terms,
    );
    patch_schema(generator, schema)
}
fn inputs_patch_schema(generator: &mut SchemaGenerator) -> Schema {
    let schema = listing_delivery::inputs_schema(generator);
    patch_schema(generator, schema)
}
fn shipping_patch_schema(generator: &mut SchemaGenerator) -> Schema {
    let (party, location) = references();
    let schema = listing_delivery::shipping_override_schema(generator, party, location);
    patch_schema(generator, schema)
}
fn collection_patch_schema(generator: &mut SchemaGenerator) -> Schema {
    let (party, location) = references();
    let schema = listing_delivery::collection_override_schema(generator, party, location);
    patch_schema(generator, schema)
}
fn optional_delivery_schema(generator: &mut SchemaGenerator) -> Schema {
    let (party, location) = references();
    let terms = crate::shipping::terms::schema(generator);
    let schema = listing_delivery::delivery_schema(
        generator,
        crate::wire::schema::object_id(ListingSourceId::PREFIX),
        crate::wire::schema::object_id(ShippingProfileId::PREFIX),
        party,
        location,
        terms,
    );
    schemars::json_schema!({"anyOf":[schema,{"type":"null"}]})
}
fn outcome_schema(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"type":"string","enum":["CHANGED","UNCHANGED"]})
}
