use crate::patch_value::PatchValue;
use serde::{Deserialize, Deserializer, Serializer};
use std::collections::HashSet;
use std::hash::Hash;

fn serialize_code<T, S>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: Serializer,
{
    serializer.serialize_str(code(*value))
}

fn deserialize_code<'de, T, D>(deserializer: D, parse: fn(&str) -> Option<T>) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| serde::de::Error::unknown_variant(&value, &[]))
}

fn serialize_option_code<T, S>(
    value: &Option<T>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(code(*value)),
        None => serializer.serialize_none(),
    }
}

fn deserialize_option_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        parse(&value)
            .map(Some)
            .ok_or_else(|| serde::de::Error::unknown_variant(&value, &[]))
    })
}

fn serialize_set_code<T, S>(
    values: &HashSet<T>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy + Eq + Hash,
    S: Serializer,
{
    serializer.collect_seq(values.iter().map(|value| code(*value)))
}

fn deserialize_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<HashSet<T>, D::Error>
where
    T: Eq + Hash,
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|value| parse(&value).ok_or_else(|| serde::de::Error::unknown_variant(&value, &[])))
        .collect()
}

fn deserialize_patch_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<PatchValue<T>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |value| {
        parse(&value)
            .map(PatchValue::Value)
            .ok_or_else(|| serde::de::Error::unknown_variant(&value, &[]))
    })
}

fn deserialize_patch_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<PatchValue<HashSet<T>>, D::Error>
where
    T: Eq + Hash,
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |values| {
        values
            .into_iter()
            .map(|value| {
                parse(&value).ok_or_else(|| serde::de::Error::unknown_variant(&value, &[]))
            })
            .collect::<Result<HashSet<_>, D::Error>>()
            .map(PatchValue::Value)
    })
}

pub(crate) mod currency {
    use super::*;
    use money::Currency;

    pub(crate) fn serialize<S>(value: &Currency, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, Currency::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Currency, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, Currency::from_code)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<Currency>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, Currency::as_str)
        }

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Currency>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_option_code(deserializer, Currency::from_code)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<Currency>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, Currency::from_code)
        }
    }
}

pub(crate) mod language {
    use super::*;
    use localization::Language;

    fn parse(value: &str) -> Option<Language> {
        match value {
            "de-DE" | "de-AT" | "de-CH" | "de-LU" | "de-LI" => Some(Language::De),
            "en-US" | "en-GB" | "en-AU" | "en-CA" | "en-NZ" | "en_IE" => Some(Language::En),
            "fr-FR" | "fr-CA" | "fr-BE" | "fr-CH" | "fr-LU" => Some(Language::Fr),
            "es-ES" | "es-MX" | "es-AR" | "es-CO" | "es-CL" | "es-PE" | "es-VE" => {
                Some(Language::Es)
            }
            "it-IT" | "it-CH" => Some(Language::It),
            "zh-CN" | "zh-Hans" => Some(Language::Zh),
            "pt-PT" | "pt-BR" => Some(Language::Pt),
            "pl-PL" => Some(Language::Pl),
            "tr-TR" => Some(Language::Tr),
            "nl-NL" | "nl-BE" => Some(Language::Nl),
            "cs-CZ" => Some(Language::Cs),
            "ja-JP" => Some(Language::Ja),
            "ru-RU" => Some(Language::Ru),
            "ar-SA" | "ar-EG" | "ar-AE" => Some(Language::Ar),
            _ => Language::from_code(value),
        }
    }

    pub(crate) fn serialize<S>(value: &Language, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, Language::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Language, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<Language>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, Language::as_str)
        }

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Language>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_option_code(deserializer, parse)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<Language>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, parse)
        }
    }
}

pub(crate) mod shop_type {
    use super::*;
    use shop_core::shop_type::ShopType;

    pub(crate) fn serialize<S>(value: &ShopType, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ShopType::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<ShopType, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, ShopType::from_code)
    }

    pub(crate) mod set {
        use super::*;

        pub(crate) fn serialize<S>(
            values: &HashSet<ShopType>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_set_code(values, serializer, ShopType::as_str)
        }

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<HashSet<ShopType>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_set_code(deserializer, ShopType::from_code)
        }
    }

    pub(crate) mod patch_set {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<HashSet<ShopType>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_set_code(deserializer, ShopType::from_code)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<ShopType>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, ShopType::from_code)
        }
    }
}

pub(crate) mod shop_partner_status {
    use super::*;
    use shop_core::partner_status::ShopPartnerStatus;

    pub(crate) fn serialize<S>(value: &ShopPartnerStatus, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ShopPartnerStatus::as_str)
    }

    pub(crate) mod set {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<HashSet<ShopPartnerStatus>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_set_code(deserializer, ShopPartnerStatus::from_code)
        }
    }
}

pub(crate) mod product_state {
    use super::*;
    use product_core::product_state::ProductState;

    fn parse(value: &str) -> Option<ProductState> {
        ProductState::from_code(value)
    }

    pub(crate) fn serialize<S>(value: &ProductState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ProductState::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<ProductState, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<ProductState>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_option_code(deserializer, parse)
        }
    }

    pub(crate) mod set {
        use super::*;

        pub(crate) fn serialize<S>(
            values: &HashSet<ProductState>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_set_code(values, serializer, parse_code)
        }

        fn parse_code(value: ProductState) -> &'static str {
            value.as_str()
        }

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<HashSet<ProductState>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_set_code(deserializer, parse)
        }
    }

    pub(crate) mod patch_set {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<HashSet<ProductState>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_set_code(deserializer, parse)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<ProductState>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, parse)
        }
    }
}

pub(crate) mod product_lifecycle {
    use super::*;
    use product_core::product_lifecycle::ProductLifecycle;

    fn parse(value: &str) -> Option<ProductLifecycle> {
        ProductLifecycle::from_code(value)
    }

    pub(crate) fn serialize<S>(value: &ProductLifecycle, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ProductLifecycle::as_str)
    }

    pub(crate) mod set {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<HashSet<ProductLifecycle>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_set_code(deserializer, parse)
        }
    }
}

pub(crate) mod prohibited_content {
    use super::*;
    use product_core::prohibited_content::ProhibitedContent;

    pub(crate) fn serialize<S>(value: &ProhibitedContent, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ProhibitedContent::as_str)
    }
}

pub(crate) mod distance_unit {
    use super::*;
    use geo::core::distance::DistanceUnit;

    fn code(value: DistanceUnit) -> &'static str {
        match value {
            DistanceUnit::Miles => "MILES",
            DistanceUnit::Yards => "YARDS",
            DistanceUnit::Feet => "FEET",
            DistanceUnit::Inches => "INCHES",
            DistanceUnit::Kilometers => "KILOMETERS",
            DistanceUnit::Meters => "METERS",
            DistanceUnit::Centimeters => "CENTIMETERS",
            DistanceUnit::Millimeters => "MILLIMETERS",
            DistanceUnit::NauticalMiles => "NAUTICAL_MILES",
        }
    }

    fn parse(value: &str) -> Option<DistanceUnit> {
        match value {
            "MILES" => Some(DistanceUnit::Miles),
            "YARDS" => Some(DistanceUnit::Yards),
            "FEET" => Some(DistanceUnit::Feet),
            "INCHES" => Some(DistanceUnit::Inches),
            "KILOMETERS" => Some(DistanceUnit::Kilometers),
            "METERS" => Some(DistanceUnit::Meters),
            "CENTIMETERS" => Some(DistanceUnit::Centimeters),
            "MILLIMETERS" => Some(DistanceUnit::Millimeters),
            "NAUTICAL_MILES" => Some(DistanceUnit::NauticalMiles),
            _ => None,
        }
    }

    pub(crate) fn serialize<S>(value: &DistanceUnit, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, code)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<DistanceUnit, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse)
    }
}

pub(crate) mod measurement_unit {
    use super::*;
    use user_core::measurement_unit::MeasurementUnit;

    fn code(value: MeasurementUnit) -> &'static str {
        match value {
            MeasurementUnit::Metric => "METRIC",
            MeasurementUnit::Imperial => "IMPERIAL",
        }
    }

    fn parse(value: &str) -> Option<MeasurementUnit> {
        match value {
            "METRIC" => Some(MeasurementUnit::Metric),
            "IMPERIAL" => Some(MeasurementUnit::Imperial),
            _ => None,
        }
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<MeasurementUnit>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, code)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<MeasurementUnit>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, parse)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;
    use product_core::product_state::ProductState;
    use serde_json::json;
    use shop_core::shop_type::ShopType;

    #[test]
    fn should_preserve_canonical_and_alias_wire_values() -> Result<(), serde_json::Error> {
        assert_eq!(
            json!("EUR"),
            serde_json::to_value(WireCurrency(Currency::Eur))?
        );
        assert_eq!(
            Language::En,
            serde_json::from_value::<WireLanguage>(json!("en-GB"))?.0
        );
        assert_eq!(
            ShopType::AuctionHouse,
            serde_json::from_value::<WireShopType>(json!("AUCTION_HOUSE"))?.0
        );
        assert_eq!(
            ProductState::Sold,
            serde_json::from_value::<WireProductState>(json!("SOLD"))?.0
        );
        assert_eq!(
            json!({
                "shopType": "AUCTION_HOUSE",
                "partnerStatus": "PARTNERED"
            }),
            serde_json::to_value(ShopCodes {
                shop_type: ShopType::AuctionHouse,
                partner_status: shop_core::partner_status::ShopPartnerStatus::Partnered,
            })?
        );
        Ok(())
    }

    #[derive(serde::Serialize)]
    #[serde(transparent)]
    struct WireCurrency(#[serde(with = "currency")] Currency);

    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct WireLanguage(#[serde(with = "language")] Language);

    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct WireShopType(#[serde(with = "shop_type")] ShopType);

    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct WireProductState(#[serde(with = "product_state")] ProductState);

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ShopCodes {
        #[serde(with = "shop_type")]
        shop_type: ShopType,
        #[serde(with = "shop_partner_status")]
        partner_status: shop_core::partner_status::ShopPartnerStatus,
    }
}
