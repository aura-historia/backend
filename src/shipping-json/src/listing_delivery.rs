//! Versioned listing configuration protocol with adapter-owned references.
use super::*;
use listing_source_core::ListingIngestionMethod;
use product_listing_core::delivery::{DeliveryIntegration, ListingDelivery};
use shipping_core::{
    inputs::{ClassMembership, FactCompleteness, ListingRateInputs},
    profile::ShippingProfileId,
    selection::{ComponentOverride, DeliveryOverrides, ProfileSelection},
};

pub trait DeliveryReferenceCodec: ShippingTermsReferenceCodec {
    fn decode_source(value: &str) -> Result<ListingSourceId, BoxError>;
    fn encode_source(value: ListingSourceId) -> String;
    fn decode_profile(value: &str) -> Result<ShippingProfileId, BoxError>;
    fn encode_profile(value: ShippingProfileId) -> String;
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum IntegrationData {
    Source {},
    Provider {
        #[schemars(schema_with = "provider_method_schema")]
        method: String,
    },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum SelectionData {
    InheritSourceDefault {},
    Explicit {
        profile_id: String,
    },
    Inline {
        #[schemars(with = "TermsData")]
        terms: serde_json::Value,
    },
    SuppressInheritance {},
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum OverrideData<T> {
    #[schemars(transform = allow_null_unit_payload)]
    Inherit,
    #[schemars(transform = allow_null_unit_payload)]
    Clear,
    Replace(T),
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InputsData {
    #[serde(deserialize_with = "required")]
    classes: RequiredNullable<ClassesData>,
    #[serde(deserialize_with = "required")]
    variants: RequiredNullable<Vec<String>>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClassesData {
    codes: Vec<String>,
    #[schemars(schema_with = "fact_completeness_schema")]
    completeness: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeliveryData {
    #[schemars(schema_with = "delivery_version_schema")]
    schema: String,
    source_id: String,
    integration: IntegrationData,
    #[serde(deserialize_with = "required")]
    selection: RequiredNullable<SelectionData>,
    rate_inputs: InputsData,
    shipping: OverrideData<PolicyData>,
    collection: OverrideData<CollectionData>,
}
pub fn decode_integration(value: serde_json::Value) -> Result<DeliveryIntegration, BoxError> {
    Ok(match serde_json::from_value::<IntegrationData>(value)? {
        IntegrationData::Source {} => DeliveryIntegration::Source,
        IntegrationData::Provider { method } => {
            let method = method.parse::<ListingIngestionMethod>()?;
            if !matches!(
                method,
                ListingIngestionMethod::Shopify | ListingIngestionMethod::Woocommerce
            ) {
                return Err(bad());
            }
            DeliveryIntegration::Provider { method }
        }
    })
}
pub fn encode_integration(value: &DeliveryIntegration) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(match value {
        DeliveryIntegration::Source => IntegrationData::Source {},
        DeliveryIntegration::Provider { method } => IntegrationData::Provider {
            method: method.as_str().into(),
        },
    })
    .map_err(box_error)
}
pub fn decode_selection<C: DeliveryReferenceCodec>(
    value: serde_json::Value,
    source: ListingSourceId,
) -> Result<ProfileSelection, BoxError> {
    Ok(match serde_json::from_value::<SelectionData>(value)? {
        SelectionData::InheritSourceDefault {} => ProfileSelection::InheritSourceDefault,
        SelectionData::SuppressInheritance {} => ProfileSelection::SuppressInheritance,
        SelectionData::Explicit { profile_id } => {
            ProfileSelection::Explicit(C::decode_profile(&profile_id)?)
        }
        SelectionData::Inline { terms } => {
            ProfileSelection::inline(super::decode::<C>(terms, source)?)
        }
    })
}
pub fn encode_selection<C: DeliveryReferenceCodec>(
    value: &ProfileSelection,
) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(match value {
        ProfileSelection::InheritSourceDefault => SelectionData::InheritSourceDefault {},
        ProfileSelection::SuppressInheritance => SelectionData::SuppressInheritance {},
        ProfileSelection::Explicit(id) => SelectionData::Explicit {
            profile_id: C::encode_profile(*id),
        },
        ProfileSelection::Inline(terms) => SelectionData::Inline {
            terms: super::encode::<C>(terms)?,
        },
    })
    .map_err(box_error)
}
pub fn decode_inputs(
    value: serde_json::Value,
    source: ListingSourceId,
) -> Result<ListingRateInputs, BoxError> {
    let data: InputsData = serde_json::from_value(value)?;
    ListingRateInputs::new(
        data.classes
            .0
            .map(|c| {
                ClassMembership::new(
                    c.codes
                        .into_iter()
                        .map(|v| SourceShippingClass::new(source, v))
                        .collect::<Result<_, _>>()?,
                    code(&c.completeness, FactCompleteness::from_code)?,
                )
                .map_err(box_error)
            })
            .transpose()?,
        data.variants
            .0
            .map(|v| {
                v.into_iter()
                    .map(|v| SourceVariant::new(source, v).map_err(box_error))
                    .collect::<Result<_, _>>()
            })
            .transpose()?,
    )
    .map_err(box_error)
}
pub fn encode_inputs(value: &ListingRateInputs) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(InputsData {
        classes: RequiredNullable(value.classes().map(|v| ClassesData {
            codes: v.classes().iter().map(|c| c.code().into()).collect(),
            completeness: v.completeness().as_str().into(),
        })),
        variants: RequiredNullable(
            value
                .variants()
                .map(|v| v.iter().map(|c| c.code().into()).collect()),
        ),
    })
    .map_err(box_error)
}
fn decode_override<T, U>(
    value: serde_json::Value,
    decode: impl FnOnce(T) -> Result<U, BoxError>,
) -> Result<ComponentOverride<U>, BoxError>
where
    T: serde::de::DeserializeOwned,
{
    Ok(match serde_json::from_value::<OverrideData<T>>(value)? {
        OverrideData::Inherit => ComponentOverride::Inherit,
        OverrideData::Clear => ComponentOverride::Clear,
        OverrideData::Replace(value) => ComponentOverride::Replace(decode(value)?),
    })
}
fn encode_override<T: Serialize, U>(
    value: &ComponentOverride<U>,
    encode: impl FnOnce(&U) -> Result<T, BoxError>,
) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(match value {
        ComponentOverride::Inherit => OverrideData::Inherit,
        ComponentOverride::Clear => OverrideData::Clear,
        ComponentOverride::Replace(value) => OverrideData::Replace(encode(value)?),
    })
    .map_err(box_error)
}
pub fn decode_shipping<C: DeliveryReferenceCodec>(
    v: serde_json::Value,
    s: ListingSourceId,
) -> Result<ComponentOverride<ShippingPolicy>, BoxError> {
    decode_override::<PolicyData, _>(v, |v| v.decode::<C>(s))
}
pub fn encode_shipping<C: DeliveryReferenceCodec>(
    v: &ComponentOverride<ShippingPolicy>,
) -> Result<serde_json::Value, BoxError> {
    encode_override(v, PolicyData::encode::<C>)
}
pub fn decode_collection<C: DeliveryReferenceCodec>(
    v: serde_json::Value,
    s: ListingSourceId,
) -> Result<ComponentOverride<CollectionPolicy>, BoxError> {
    decode_override::<CollectionData, _>(v, |v| v.decode::<C>(s))
}
pub fn encode_collection<C: DeliveryReferenceCodec>(
    v: &ComponentOverride<CollectionPolicy>,
) -> Result<serde_json::Value, BoxError> {
    encode_override(v, CollectionData::encode::<C>)
}
pub fn decode<C: DeliveryReferenceCodec>(
    v: serde_json::Value,
) -> Result<ListingDelivery, BoxError> {
    // Fixed schema requires every nullable field and rejects unexpected fields.
    let d: DeliveryData = serde_json::from_value(v)?;
    if d.schema != "LISTING_DELIVERY_V1" {
        return Err(bad());
    }
    let source = C::decode_source(&d.source_id)?;
    ListingDelivery::new(
        source,
        decode_integration(serde_json::to_value(d.integration)?)?,
        d.selection
            .0
            .map(|v| decode_selection::<C>(serde_json::to_value(v)?, source))
            .transpose()?,
        decode_inputs(serde_json::to_value(d.rate_inputs)?, source)?,
        DeliveryOverrides {
            shipping: decode_shipping::<C>(serde_json::to_value(d.shipping)?, source)?,
            collection: decode_collection::<C>(serde_json::to_value(d.collection)?, source)?,
        },
    )
    .map_err(box_error)
}
pub fn encode<C: DeliveryReferenceCodec>(
    v: &ListingDelivery,
) -> Result<serde_json::Value, BoxError> {
    Ok(
        serde_json::json!({"schema":"LISTING_DELIVERY_V1", "source_id":C::encode_source(v.source_id()),
            "integration":encode_integration(v.integration())?,
            "selection":v.selection().map(encode_selection::<C>).transpose()?,
            "rate_inputs":encode_inputs(v.rate_inputs())?,
            "shipping":encode_shipping::<C>(&v.overrides().shipping)?,
            "collection":encode_collection::<C>(&v.overrides().collection)?,
        }),
    )
}

/// Schema generated from the integration codec's DTO rather than a parallel transport model.
pub fn integration_schema(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<IntegrationData>()
}
pub fn provider_method_schema(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"type":"string","enum":[ListingIngestionMethod::Shopify.as_str(),ListingIngestionMethod::Woocommerce.as_str()]})
}

/// Serde accepts either omitted content or explicit null for adjacent unit variants.
pub fn allow_null_unit_payload(schema: &mut Schema) {
    if let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        properties.insert("value".into(), serde_json::json!({"type":"null"}));
    }
}

fn delivery_version_schema(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"type":"string","const":"LISTING_DELIVERY_V1"})
}
fn fact_completeness_schema(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({"type":"string","enum":[FactCompleteness::Complete.as_str(), FactCompleteness::Partial.as_str()]})
}
/// The existing delivery DTOs own schema vocabulary; adapters supply their object references.
pub fn selection_schema(generator: &mut SchemaGenerator, profile: Schema, terms: Schema) -> Schema {
    let result = generator.subschema_for::<SelectionData>();
    super::schema::references(generator, &[("profile_id", profile), ("terms", terms)]);
    result
}
pub fn inputs_schema(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<InputsData>()
}
pub fn shipping_override_schema(
    generator: &mut SchemaGenerator,
    party: Schema,
    location: Schema,
) -> Schema {
    let result = generator.subschema_for::<OverrideData<PolicyData>>();
    super::schema::references(
        generator,
        &[("party_id", party), ("party_location_id", location)],
    );
    result
}
pub fn collection_override_schema(
    generator: &mut SchemaGenerator,
    party: Schema,
    location: Schema,
) -> Schema {
    let result = generator.subschema_for::<OverrideData<CollectionData>>();
    super::schema::references(
        generator,
        &[("party_id", party), ("party_location_id", location)],
    );
    result
}
pub fn delivery_schema(
    generator: &mut SchemaGenerator,
    source: Schema,
    profile: Schema,
    party: Schema,
    location: Schema,
    terms: Schema,
) -> Schema {
    let result = generator.subschema_for::<DeliveryData>();
    super::schema::references(
        generator,
        &[
            ("source_id", source),
            ("profile_id", profile),
            ("terms", terms),
            ("party_id", party),
            ("party_location_id", location),
        ],
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_scope_uses_only_the_enabled_native_method() {
        for method in ["SHOPIFY", "WOOCOMMERCE"] {
            let canonical = serde_json::json!({"kind":"PROVIDER","method":method});
            let integration = decode_integration(canonical.clone()).unwrap();
            assert_eq!(canonical, encode_integration(&integration).unwrap());
            let mut invalid = canonical;
            invalid["domain"] = serde_json::json!("synthetic.example");
            assert!(decode_integration(invalid).is_err());
        }
        assert!(
            decode_integration(serde_json::json!({"kind":"PROVIDER","method":"WEB_CRAWL"}))
                .is_err()
        );
    }
}
