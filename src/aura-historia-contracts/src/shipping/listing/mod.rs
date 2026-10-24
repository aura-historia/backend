//! Purpose-specific public read projections of listing shipping, shared by REST and MCP.
//! Collection items receive bounded summaries; single-listing details receive clean advertised
//! terms. Absent optional values are omitted at every level, never serialized as null.
mod advertised_terms;
mod assessment;
mod detail;
mod summary;
mod values;

pub use detail::{ListingShippingDetailData, PersonalizedShippingDetailData};
pub use summary::{ListingShippingSummaryData, PersonalizedShippingSummaryData};

use schemars::JsonSchema;
use serde::Serialize;
use time::OffsetDateTime;

/// A public listing shipping representation selected by the response's purpose.
pub trait ListingShippingProjection: Sized {
    type Source;
    /// `None` when the representation would carry no information.
    fn project(source: Self::Source) -> Option<Self>;
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShippingMatchValuationData {
    interpretation: &'static str,
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    fx_rate_id: fxrate_core::FxRateId,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    captured_at: OffsetDateTime,
    maximum_per_item: ShippingMatchMaximumData,
}
#[derive(Debug, Serialize, JsonSchema)]
struct ShippingMatchMaximumData {
    #[schemars(range(max = u64::MAX))]
    amount: u64,
    #[serde(with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: money::Currency,
}
impl From<search_filter_core::ShippingMatchValuation> for ShippingMatchValuationData {
    fn from(v: search_filter_core::ShippingMatchValuation) -> Self {
        Self {
            interpretation:
                product_listing_core::shipping_search::ShippingSearch::FX_INTERPRETATION,
            fx_rate_id: v.fx_rate_id,
            captured_at: v.captured_at,
            maximum_per_item: ShippingMatchMaximumData {
                amount: v.maximum_per_item.monetary_amount.into(),
                currency: v.maximum_per_item.currency,
            },
        }
    }
}
