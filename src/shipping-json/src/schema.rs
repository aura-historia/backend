//! Canonical code descriptions for the terms codec's Serde fields.
use super::*;
use schemars::json_schema;
fn codes(values: impl IntoIterator<Item = &'static str>) -> Schema {
    json_schema!({"type":"string","enum":values.into_iter().collect::<Vec<_>>()})
}
fn optional(schema: Schema) -> Schema {
    json_schema!({"anyOf":[schema,{"type":"null"}]})
}
macro_rules! canonical {
    ($name:ident,$ty:ident,[$($variant:ident),+])=>{
        pub(super) fn $name(_: &mut SchemaGenerator)->Schema {codes([$($ty::$variant.as_str()),+])}
    };
    (optional $name:ident,$ty:ident,[$($variant:ident),+])=>{
        pub(super) fn $name(_: &mut SchemaGenerator)->Schema {optional(codes([$($ty::$variant.as_str()),+]))}
    };
}
canonical!(completeness, CoverageCompleteness, [Complete, Partial]);
canonical!(postal_version, PostalRuleVersion, [V1]);
canonical!(dependency, CollectionDependency, [Independent, Shipping]);
canonical!(time_unit, TimeUnit, [Hours, Days, Weeks]);
canonical!(scope, QualifyingScope, [Item, Lot, Shipment, SellerOrder]);
canonical!(optional optional_scope,QualifyingScope,[Item,Lot,Shipment,SellerOrder]);
canonical!(optional eligibility,ShippingEligibility,[Offered,NotOffered,SubjectToConfirmation]);
canonical!(optional charge_basis,ChargeBasis,[PerItem,PerLot,PerShipment,PerSellerOrder]);
canonical!(optional time_basis,TimeBasis,[Calendar,Business]);
canonical!(optional trigger,TimeTrigger,[Order,OrderConfirmation,Payment,Dispatch,ReadyForCollection]);
pub(super) fn version(_: &mut SchemaGenerator) -> Schema {
    codes(["SHIP_TERMS_V1"])
}
pub(super) fn release(_: &mut SchemaGenerator) -> Schema {
    codes([ReferenceRelease::CURRENT.as_str()])
}
pub(super) fn money_currency(_: &mut SchemaGenerator) -> Schema {
    use strum::IntoEnumIterator;
    codes(Currency::iter().map(Currency::as_str))
}
pub(super) fn currency(generator: &mut SchemaGenerator) -> Schema {
    optional(money_currency(generator))
}
pub(super) fn country(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"string","enum":ReferenceRelease::CURRENT.country_codes()})
}
pub(super) fn subdivision(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"string","pattern":"^[A-Z]{2}-[A-Z0-9]{1,3}$"})
}
pub(super) fn geography(generator: &mut SchemaGenerator) -> Schema {
    optional(
        json_schema!({"type":"object","additionalProperties":false,"required":["reference_release","address_text","country","subdivision","postal_code"],"properties":{
            "reference_release":release(generator),
            "address_text":{"type":["string","null"]},
            "country":optional(country(generator)),
            "subdivision":optional(subdivision(generator)),
            "postal_code":{"type":["string","null"]}
        }}),
    )
}
pub(super) fn position(_: &mut SchemaGenerator) -> Schema {
    use geo::SpatialPrecision;
    optional(
        json_schema!({"type":"object","additionalProperties":false,"required":["point","precision","accuracy_metres"],"properties":{
            "point":{"type":"object","additionalProperties":false,"required":["latitude_degrees","longitude_degrees"],"properties":{
                "latitude_degrees":{"type":"number","minimum":-90,"maximum":90},
                "longitude_degrees":{"type":"number","minimum":-180,"maximum":180}
            }},
            "precision":codes([SpatialPrecision::Premises.as_str(),SpatialPrecision::Street.as_str(),SpatialPrecision::Locality.as_str(),SpatialPrecision::Representative.as_str()]),
            "accuracy_metres":{"type":["number","null"],"minimum":0}
        }}),
    )
}
pub(super) fn features(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"array","items":codes([ServiceFeature::Tracking.as_str(),ServiceFeature::Insured.as_str(),ServiceFeature::WhiteGlove.as_str(),ServiceFeature::Installation.as_str(),ServiceFeature::Packing.as_str()])})
}
pub(super) fn components(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"object","propertyNames":codes([ChargeComponent::Transport.as_str(),ChargeComponent::Packing.as_str(),ChargeComponent::Insurance.as_str(),ChargeComponent::Handling.as_str(),ChargeComponent::Installation.as_str(),ChargeComponent::ImportDuty.as_str(),ChargeComponent::Tax.as_str()]),"additionalProperties":codes([ComponentInclusion::Included.as_str(),ComponentInclusion::Excluded.as_str()])})
}

/// Replace adapter-owned object references in schemas generated from the codec DTOs.
pub(super) fn references(generator: &mut SchemaGenerator, references: &[(&str, Schema)]) {
    fn replace(value: &mut serde_json::Value, references: &[(&str, Schema)]) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(properties) = object
                    .get_mut("properties")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    for (name, schema) in references {
                        if let Some(value) = properties.get_mut(*name) {
                            *value = serde_json::to_value(schema).expect("schema is JSON");
                        }
                    }
                }
                for value in object.values_mut() {
                    replace(value, references);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    replace(value, references);
                }
            }
            _ => {}
        }
    }
    for definition in generator.definitions_mut().values_mut() {
        replace(definition, references);
    }
}
