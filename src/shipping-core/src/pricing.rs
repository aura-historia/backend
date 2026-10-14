use crate::InvalidShippingValue;
use money::{Currency, Price};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChargeBasis {
    PerItem,
    PerLot,
    PerShipment,
    PerSellerOrder,
}
codes!(ChargeBasis, PerItem => "PER_ITEM", PerLot => "PER_LOT", PerShipment => "PER_SHIPMENT", PerSellerOrder => "PER_SELLER_ORDER");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QualifyingScope {
    Item,
    Lot,
    Shipment,
    SellerOrder,
}
codes!(QualifyingScope, Item => "ITEM", Lot => "LOT", Shipment => "SHIPMENT", SellerOrder => "SELLER_ORDER");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChargeComponent {
    Transport,
    Packing,
    Insurance,
    Handling,
    Installation,
    ImportDuty,
    Tax,
}
codes!(ChargeComponent, Transport => "TRANSPORT", Packing => "PACKING", Insurance => "INSURANCE", Handling => "HANDLING", Installation => "INSTALLATION", ImportDuty => "IMPORT_DUTY", Tax => "TAX");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentInclusion {
    Included,
    Excluded,
}
codes!(ComponentInclusion, Included => "INCLUDED", Excluded => "EXCLUDED");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceRange {
    minimum: Price,
    maximum: Price,
}
impl PriceRange {
    pub fn new(minimum: Price, maximum: Price) -> Result<Self, InvalidShippingValue> {
        if minimum.currency != maximum.currency {
            return Err(InvalidShippingValue::CurrencyMismatch);
        }
        if minimum.monetary_amount > maximum.monetary_amount {
            return Err(InvalidShippingValue::Bounds);
        }
        Ok(Self { minimum, maximum })
    }
    pub fn minimum(self) -> Price {
        self.minimum
    }
    pub fn maximum(self) -> Price {
        self.maximum
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShippingPrice {
    Free(Option<Currency>),
    Fixed(Price),
    From(Price),
    EstimatedRange(PriceRange),
    ExternallyCalculated(Option<Currency>),
    OnRequest(Option<Currency>),
}
impl ShippingPrice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free(_) => "FREE",
            Self::Fixed(_) => "FIXED",
            Self::From(_) => "FROM",
            Self::EstimatedRange(_) => "ESTIMATED_RANGE",
            Self::ExternallyCalculated(_) => "EXTERNALLY_CALCULATED",
            Self::OnRequest(_) => "ON_REQUEST",
        }
    }
    pub fn currency(self) -> Option<Currency> {
        match self {
            Self::Free(currency)
            | Self::ExternallyCalculated(currency)
            | Self::OnRequest(currency) => currency,
            Self::Fixed(price) | Self::From(price) => Some(price.currency),
            Self::EstimatedRange(range) => Some(range.minimum().currency),
        }
    }
}

/// Known components annotate the advertised charge. Absence is no inclusion assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingRate {
    price: ShippingPrice,
    basis: Option<ChargeBasis>,
    qualifying_scope: Option<QualifyingScope>,
    components: BTreeMap<ChargeComponent, ComponentInclusion>,
}
impl ShippingRate {
    pub fn new(
        price: ShippingPrice,
        basis: Option<ChargeBasis>,
        qualifying_scope: Option<QualifyingScope>,
        components: BTreeMap<ChargeComponent, ComponentInclusion>,
    ) -> Self {
        Self {
            price,
            basis,
            qualifying_scope,
            components,
        }
    }
    pub fn price(&self) -> ShippingPrice {
        self.price
    }
    pub fn basis(&self) -> Option<ChargeBasis> {
        self.basis
    }
    pub fn qualifying_scope(&self) -> Option<QualifyingScope> {
        self.qualifying_scope
    }
    pub fn components(&self) -> &BTreeMap<ChargeComponent, ComponentInclusion> {
        &self.components
    }
}
