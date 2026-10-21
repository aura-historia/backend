//! Explicit wire-shape annotations for the existing custom shipping serializers.
//! Terms use the codec's actual DTO schema; these markers describe only the public envelope.
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Serialize;

fn terms(generator: &mut SchemaGenerator) -> Schema {
    shipping_json::terms_schema(
        generator,
        crate::wire::schema::object_id(party_core::party_id::PartyId::PREFIX),
        crate::wire::schema::object_id(party_core::party_location_id::PartyLocationId::PREFIX),
    )
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdvertisedShippingSchema {
    #[schemars(schema_with = "terms")]
    pub terms: serde_json::Value,
    pub display: Option<DisplaySchema>,
    #[schemars(range(max = u64::MAX))]
    pub listing_revision: u64,
    #[schemars(range(max = u64::MAX))]
    pub source_revision: u64,
    #[schemars(range(max = u64::MAX))]
    pub default_revision: Option<u64>,
    pub profile: Option<ProfileSchema>,
    pub listing_evidence: Option<EvidenceSchema>,
    pub evaluated_at: String,
    pub shipping_reasons: Vec<ReasonSchema>,
    pub collection_reasons: Vec<ReasonSchema>,
    pub notices: Vec<String>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySchema {
    #[schemars(schema_with = "crate::wire::currency::schema")]
    #[serde(with = "crate::wire::currency")]
    pub currency: money::Currency,
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    pub fx_rate_id: fxrate_core::FxRateId,
    pub captured_at: String,
    pub rates: Vec<RateSchema>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateSchema {
    #[schemars(range(max = usize::MAX))]
    pub zone_index: usize,
    #[schemars(range(max = usize::MAX))]
    pub service_index: usize,
    pub price: Option<PriceSchema>,
    pub basis: Option<String>,
}
#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PriceSchema {
    Free {
        currency: Option<String>,
    },
    Fixed {
        amount: MoneySchema,
    },
    From {
        amount: MoneySchema,
    },
    EstimatedRange {
        minimum: MoneySchema,
        maximum: MoneySchema,
    },
}
#[derive(Serialize, JsonSchema)]
pub struct MoneySchema {
    #[schemars(range(max = u64::MAX))]
    pub amount_minor: u64,
    pub currency: String,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSchema {
    #[schemars(schema_with = "profile_id")]
    pub shipping_profile_id: shipping_core::profile::ShippingProfileId,
    #[schemars(range(max = u64::MAX))]
    pub terms_revision: u64,
    #[schemars(range(max = u64::MAX))]
    pub management_revision: u64,
    pub lifecycle: String,
    pub evidence: EvidenceSchema,
}
fn profile_id(_: &mut SchemaGenerator) -> Schema {
    crate::wire::schema::object_id(shipping_core::profile::ShippingProfileId::PREFIX)
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceSchema {
    pub verified_at: String,
}
#[derive(Serialize, JsonSchema)]
pub struct ReasonSchema {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = usize::MAX))]
    pub index: Option<usize>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalizedShippingSchema {
    pub destination: GeographySchema,
    pub attribution: Option<AttributionSchema>,
    pub evaluated_at: String,
    pub eligibility: Option<String>,
    #[schemars(range(max = usize::MAX))]
    pub selected_zone: Option<usize>,
    pub options: Vec<ServiceSchema>,
    pub reasons: Vec<ReasonSchema>,
    pub collection: CollectionSchema,
    pub binding_quote: bool,
    pub complete_landed_cost: bool,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GeographySchema {
    pub reference_release: String,
    pub country: Option<String>,
    pub subdivision: Option<String>,
    pub postal_code: Option<String>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttributionSchema {
    pub provider: String,
    pub attribution_url: String,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSchema {
    #[schemars(range(max = usize::MAX))]
    pub service_index: usize,
    pub applicability: String,
    pub eligibility: Option<String>,
    pub charge: Option<ChargeSchema>,
    pub reasons: Vec<ReasonSchema>,
}
#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChargeSchema {
    Fixed {
        amount: MoneySchema,
    },
    From {
        amount: MoneySchema,
    },
    EstimatedRange {
        minimum: MoneySchema,
        maximum: MoneySchema,
    },
}
#[derive(Serialize, JsonSchema)]
pub struct CollectionSchema {
    pub eligibility: Option<String>,
    pub applicability: String,
    pub reasons: Vec<ReasonSchema>,
}
