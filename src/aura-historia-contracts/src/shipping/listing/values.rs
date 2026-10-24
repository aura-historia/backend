//! Semantic leaf values shared by the listing shipping read projections.
use fxrate_core::FxRateId;
use money::{Currency, Price};
use product_listing_service::shipping::ShippingDisplay;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::Serialize;
use shipping_core::{
    assessment::AssessedCharge,
    pricing::{ChargeBasis, QualifyingScope, ShippingPrice},
    terms::ShippingEligibility,
};
use time::OffsetDateTime;

pub(super) fn codes(values: impl IntoIterator<Item = &'static str>) -> Schema {
    json_schema!({"type": "string", "enum": values.into_iter().collect::<Vec<_>>()})
}
pub(super) fn eligibility(_: &mut SchemaGenerator) -> Schema {
    use ShippingEligibility::*;
    codes([Offered, NotOffered, SubjectToConfirmation].map(ShippingEligibility::as_str))
}
pub(super) fn charge_basis(_: &mut SchemaGenerator) -> Schema {
    use ChargeBasis::*;
    codes([PerItem, PerLot, PerShipment, PerSellerOrder].map(ChargeBasis::as_str))
}
pub(super) fn qualifying_scope(_: &mut SchemaGenerator) -> Schema {
    use QualifyingScope::*;
    codes([Item, Lot, Shipment, SellerOrder].map(QualifyingScope::as_str))
}
pub(super) fn reference_release(_: &mut SchemaGenerator) -> Schema {
    codes([geo::reference::ReferenceRelease::CURRENT.as_str()])
}

/// Advertised amount in currency minor units; never an authorized quote or checkout total.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ShippingMoneyData")]
pub struct MoneyData {
    #[schemars(range(max = u64::MAX))]
    amount_minor: u64,
    #[serde(with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: Currency,
}
impl From<Price> for MoneyData {
    fn from(price: Price) -> Self {
        Self {
            amount_minor: price.monetary_amount.into(),
            currency: price.currency,
        }
    }
}

/// Every advertised price kind. On-request and externally calculated prices stay explicit
/// rather than becoming an absent or zero price.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "ListingShippingPriceData")]
pub enum PriceData {
    Free {
        #[serde(
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::currency::option"
        )]
        #[schemars(schema_with = "crate::wire::currency::schema")]
        currency: Option<Currency>,
    },
    Fixed {
        amount: MoneyData,
    },
    From {
        amount: MoneyData,
    },
    EstimatedRange {
        minimum: MoneyData,
        maximum: MoneyData,
    },
    ExternallyCalculated {
        #[serde(
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::currency::option"
        )]
        #[schemars(schema_with = "crate::wire::currency::schema")]
        currency: Option<Currency>,
    },
    OnRequest {
        #[serde(
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::currency::option"
        )]
        #[schemars(schema_with = "crate::wire::currency::schema")]
        currency: Option<Currency>,
    },
}
impl From<ShippingPrice> for PriceData {
    fn from(price: ShippingPrice) -> Self {
        match price {
            ShippingPrice::Free(currency) => Self::Free { currency },
            ShippingPrice::Fixed(amount) => Self::Fixed {
                amount: amount.into(),
            },
            ShippingPrice::From(amount) => Self::From {
                amount: amount.into(),
            },
            ShippingPrice::EstimatedRange(range) => Self::EstimatedRange {
                minimum: range.minimum().into(),
                maximum: range.maximum().into(),
            },
            ShippingPrice::ExternallyCalculated(currency) => {
                Self::ExternallyCalculated { currency }
            }
            ShippingPrice::OnRequest(currency) => Self::OnRequest { currency },
        }
    }
}

/// The requested-currency value of an advertised amount, only when it is in another currency.
/// Free, on-request and externally calculated prices have no amount to convert.
pub(super) fn conversion(
    advertised: ShippingPrice,
    display: Option<ShippingPrice>,
) -> Option<ShippingPrice> {
    match advertised {
        ShippingPrice::Fixed(_) | ShippingPrice::From(_) | ShippingPrice::EstimatedRange(_) => {
            display.filter(|display| display.currency() != advertised.currency())
        }
        ShippingPrice::Free(_)
        | ShippingPrice::ExternallyCalculated(_)
        | ShippingPrice::OnRequest(_) => None,
    }
}

/// Arithmetic advertised charge for an explicit basis and count; never a quote or landed cost.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "DeliveryAssessmentCharge")]
pub enum ChargeData {
    Fixed {
        amount: MoneyData,
    },
    From {
        amount: MoneyData,
    },
    EstimatedRange {
        minimum: MoneyData,
        maximum: MoneyData,
    },
}
impl From<AssessedCharge> for ChargeData {
    fn from(charge: AssessedCharge) -> Self {
        match charge {
            AssessedCharge::Fixed(amount) => Self::Fixed {
                amount: amount.into(),
            },
            AssessedCharge::From(amount) => Self::From {
                amount: amount.into(),
            },
            AssessedCharge::EstimatedRange(range) => Self::EstimatedRange {
                minimum: range.minimum().into(),
                maximum: range.maximum().into(),
            },
        }
    }
}

/// Shared valuation of the requested-currency values in one shipping object. Present only
/// when a value was actually converted; HalfUp rounding uses currency minor-unit exponents.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "ShippingDisplayData")]
pub struct DisplayData {
    #[serde(with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: Currency,
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    fx_rate_id: FxRateId,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    captured_at: OffsetDateTime,
}
impl From<&ShippingDisplay> for DisplayData {
    fn from(display: &ShippingDisplay) -> Self {
        Self {
            currency: display.currency,
            fx_rate_id: display.fx_rate_id,
            captured_at: display.captured_at,
        }
    }
}
