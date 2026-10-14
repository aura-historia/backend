//! Versioned listing configuration protocol with adapter-owned references.
use super::*;
use listing_source_core::{Domain, ListingIngestionMethod};
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
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum IntegrationData {
    Source {},
    Provider { method: String, domain: String },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum SelectionData {
    InheritSourceDefault {},
    Explicit { profile_id: String },
    Inline { terms: serde_json::Value },
    SuppressInheritance {},
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum OverrideData<T> {
    Inherit,
    Clear,
    Replace(T),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputsData {
    #[serde(deserialize_with = "required")]
    classes: RequiredNullable<ClassesData>,
    #[serde(deserialize_with = "required")]
    variants: RequiredNullable<Vec<String>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassesData {
    codes: Vec<String>,
    completeness: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryData {
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
        IntegrationData::Provider { method, domain } => {
            let parsed = Domain::try_from(domain.as_str())?;
            if parsed.as_str() != domain || domain.len() > 253 {
                return Err(bad());
            }
            DeliveryIntegration::Provider {
                method: method.parse::<ListingIngestionMethod>()?,
                domain: parsed,
            }
        }
    })
}
pub fn encode_integration(value: &DeliveryIntegration) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(match value {
        DeliveryIntegration::Source => IntegrationData::Source {},
        DeliveryIntegration::Provider { method, domain } => IntegrationData::Provider {
            method: method.as_str().into(),
            domain: domain.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_domains_must_be_canonical_in_versioned_protocols() {
        let canonical = serde_json::json!({
            "kind": "PROVIDER",
            "method": "SHOPIFY",
            "domain": "synthetic.myshopify.com",
        });
        let integration = decode_integration(canonical.clone()).unwrap();
        assert_eq!(canonical, encode_integration(&integration).unwrap());
        for domain in ["Synthetic.MyShopify.com", "synthetic。myshopify.com"] {
            let mut invalid = canonical.clone();
            invalid["domain"] = serde_json::json!(domain);
            assert!(decode_integration(invalid).is_err());
        }
    }
}
