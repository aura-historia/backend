//! Public delivery commands, results and pure codecs shared by transports and ingestion.
pub use crate::shipping::terms::PublicReferences as PublicDeliveryReferences;
use application::{
    error::{BoxError, box_error, static_error},
    patch_field::PatchField,
};
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_service::{
    ports::ProductListingStorageVersion,
    use_cases::{ChangeProductListingDeliveryCommand, ProductListingDeliveryPatch},
};
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
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandData {
    expected_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import: Option<ImportData>,
    delivery: WirePatch<PatchData>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchData {
    integration: Value,
    selection: WirePatch<Value>,
    rate_inputs: WirePatch<Value>,
    shipping: WirePatch<Value>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum WirePatch<T> {
    Unchanged,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryData {
    pub product_listing_id: product_listing_core::product_listing_id::ProductListingId,
    pub expected_version: u64,
    pub delivery: Option<Value>,
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
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationData {
    pub product_listing_id: product_listing_core::product_listing_id::ProductListingId,
    pub expected_version: u64,
    pub outcome: &'static str,
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
