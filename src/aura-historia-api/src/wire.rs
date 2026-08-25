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

fn deserialize_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| invalid_code(&value, expected))
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
    expected: Option<&'static [&'static str]>,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        parse(&value)
            .map(Some)
            .ok_or_else(|| invalid_code(&value, expected))
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
    expected: Option<&'static [&'static str]>,
) -> Result<HashSet<T>, D::Error>
where
    T: Eq + Hash,
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|value| parse(&value).ok_or_else(|| invalid_code(&value, expected)))
        .collect()
}

fn deserialize_patch_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<PatchValue<T>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |value| {
        parse(&value)
            .map(PatchValue::Value)
            .ok_or_else(|| invalid_code(&value, expected))
    })
}

fn deserialize_patch_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<PatchValue<HashSet<T>>, D::Error>
where
    T: Eq + Hash,
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |values| {
        values
            .into_iter()
            .map(|value| parse(&value).ok_or_else(|| invalid_code(&value, expected)))
            .collect::<Result<HashSet<_>, D::Error>>()
            .map(PatchValue::Value)
    })
}

fn invalid_code<E>(value: &str, expected: Option<&'static [&'static str]>) -> E
where
    E: serde::de::Error,
{
    match expected {
        Some(expected) => E::unknown_variant(value, expected),
        None => E::custom(format!("unsupported code `{value}`")),
    }
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
        deserialize_code(deserializer, Currency::from_code, None)
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
            deserialize_option_code(deserializer, Currency::from_code, None)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<Currency>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, Currency::from_code, None)
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
        deserialize_code(deserializer, parse, None)
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
            deserialize_option_code(deserializer, parse, None)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<Language>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, parse, None)
        }
    }
}

pub(crate) mod notification_kind {
    use super::*;
    use notification_core::notification_kind::NotificationKind;

    pub(crate) fn serialize<S>(value: &NotificationKind, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, NotificationKind::as_str)
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
        deserialize_code(deserializer, ShopType::from_code, None)
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
            deserialize_set_code(deserializer, ShopType::from_code, None)
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
            deserialize_patch_set_code(deserializer, ShopType::from_code, None)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<ShopType>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, ShopType::from_code, None)
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
            deserialize_set_code(deserializer, ShopPartnerStatus::from_code, None)
        }
    }
}

pub(crate) mod listing_availability {
    use super::*;
    use product_listing_core::listing_availability::ListingAvailability;

    fn parse(value: &str) -> Option<ListingAvailability> {
        ListingAvailability::from_code(value)
    }

    pub(crate) fn serialize<S>(
        value: &ListingAvailability,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ListingAvailability::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<ListingAvailability, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse, None)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<ListingAvailability>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, ListingAvailability::as_str)
        }

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<ListingAvailability>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_option_code(deserializer, parse, None)
        }
    }

    pub(crate) mod set {
        use super::*;

        pub(crate) fn serialize<S>(
            values: &HashSet<ListingAvailability>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_set_code(values, serializer, ListingAvailability::as_str)
        }

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<HashSet<ListingAvailability>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_set_code(deserializer, parse, None)
        }
    }

    pub(crate) mod patch_set {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<HashSet<ListingAvailability>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_set_code(deserializer, parse, None)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<ListingAvailability>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, parse, None)
        }
    }
}

pub(crate) mod listing_lifecycle {
    use super::*;
    use product_listing_core::listing_lifecycle::ListingLifecycle;

    pub(crate) fn serialize<S>(value: &ListingLifecycle, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ListingLifecycle::as_str)
    }
}

pub(crate) mod prohibited_content {
    use super::*;
    use product_listing_core::prohibited_content::ProhibitedContent;

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

    pub(crate) fn serialize<S>(value: &DistanceUnit, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, DistanceUnit::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<DistanceUnit, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, DistanceUnit::from_code, None)
    }
}

pub(crate) mod measurement_unit {
    use super::*;
    use user_core::measurement_unit::MeasurementUnit;

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<MeasurementUnit>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, MeasurementUnit::as_str)
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
            deserialize_patch_code(deserializer, MeasurementUnit::from_code, None)
        }
    }
}

pub(crate) mod user_tier {
    use super::*;
    use user_core::tier::UserTier;

    pub(crate) fn serialize<S>(value: &UserTier, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, UserTier::as_str)
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<UserTier>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, UserTier::from_code, None)
        }
    }
}

pub(crate) mod user_role {
    use super::*;
    use user_core::role::UserRole;

    pub(crate) fn serialize<S>(value: &UserRole, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, UserRole::as_str)
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<UserRole>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, UserRole::from_code, None)
        }
    }
}

pub(crate) mod search_filter_state {
    use super::*;
    use search_filter_core::search_filter_state::SearchFilterState;

    pub(crate) fn serialize<S>(value: &SearchFilterState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, SearchFilterState::as_str)
    }
}

pub(crate) mod watchlist_state {
    use super::*;
    use watchlist_core::watchlist_state::WatchlistState;

    pub(crate) fn serialize<S>(value: &WatchlistState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, WatchlistState::as_str)
    }
}

pub(crate) mod partner_shop_application_state {
    use super::*;
    use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;

    pub(crate) fn serialize<S>(
        value: &PartnerShopApplicationState,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, PartnerShopApplicationState::as_str)
    }
}

pub(crate) mod partner_application_decision {
    use super::*;
    use notification_core::notification::PartnerApplicationDecision;

    fn code(value: PartnerApplicationDecision) -> &'static str {
        match value {
            PartnerApplicationDecision::Approved => "APPROVED",
            PartnerApplicationDecision::Rejected => "REJECTED",
        }
    }

    pub(crate) fn serialize<S>(
        value: &PartnerApplicationDecision,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, code)
    }
}

pub(crate) mod billing_plan {
    use super::*;
    use billing_service::use_cases::BillingPlan;

    const EXPECTED: &[&str] = &["PRO", "ULTIMATE"];

    fn parse(value: &str) -> Option<BillingPlan> {
        match value {
            "PRO" => Some(BillingPlan::Pro),
            "ULTIMATE" => Some(BillingPlan::Ultimate),
            _ => None,
        }
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<BillingPlan, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse, Some(EXPECTED))
    }
}

pub(crate) mod billing_cycle {
    use super::*;
    use billing_service::use_cases::BillingCycle;

    const EXPECTED: &[&str] = &["MONTHLY", "YEARLY"];

    fn parse(value: &str) -> Option<BillingCycle> {
        match value {
            "MONTHLY" => Some(BillingCycle::Monthly),
            "YEARLY" => Some(BillingCycle::Yearly),
            _ => None,
        }
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<BillingCycle, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse, Some(EXPECTED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::core::distance::DistanceUnit;
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_lifecycle::ProductLifecycle;
    use product_listing_core::product_state::ProductState;
    use product_listing_core::prohibited_content::ProhibitedContent;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use shop_core::partner_status::ShopPartnerStatus;
    use shop_core::shop_type::ShopType;
    use std::collections::HashSet;
    use user_core::measurement_unit::MeasurementUnit;

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
                partner_status: ShopPartnerStatus::Partnered,
            })?
        );
        assert_eq!(
            json!({
                "productState": "SOLD",
                "productLifecycle": "DELETED",
                "distanceUnit": "KILOMETERS",
                "measurementUnit": "METRIC",
                "prohibitedContent": "NONE"
            }),
            serde_json::to_value(CompatibilityCodes {
                product_state: ProductState::Sold,
                product_lifecycle: ProductLifecycle::Deleted,
                distance_unit: DistanceUnit::Kilometers,
                measurement_unit: Some(MeasurementUnit::Metric),
                prohibited_content: ProhibitedContent::None,
            })?
        );
        Ok(())
    }

    #[test]
    fn should_decode_option_code_defaults_null_alias_and_invalid_values()
    -> Result<(), serde_json::Error> {
        assert_eq!(
            None,
            serde_json::from_str::<WireCurrencyOption>("{}")?.value
        );
        assert_eq!(
            None,
            serde_json::from_str::<WireCurrencyOption>(r#"{"value":null}"#)?.value
        );
        assert_eq!(
            Some(Currency::Eur),
            serde_json::from_str::<WireCurrencyOption>(r#"{"value":"EUR"}"#)?.value
        );
        assert_eq!(
            Some(Language::En),
            serde_json::from_str::<WireLanguageOption>(r#"{"value":"en-GB"}"#)?.value
        );

        let error = error_text::<WireCurrencyOption>(json!({"value": "NOPE"}));
        assert!(error.contains("NOPE"));
        assert!(error.contains("unsupported code"));
        Ok(())
    }

    #[test]
    fn should_decode_patch_code_omitted_null_value_and_invalid_values()
    -> Result<(), serde_json::Error> {
        assert_eq!(
            PatchValue::Omitted,
            serde_json::from_str::<WireCurrencyPatch>("{}")?.value
        );
        assert_eq!(
            PatchValue::Null,
            serde_json::from_str::<WireCurrencyPatch>(r#"{"value":null}"#)?.value
        );
        assert_eq!(
            PatchValue::Value(Currency::Eur),
            serde_json::from_str::<WireCurrencyPatch>(r#"{"value":"EUR"}"#)?.value
        );

        let error = error_text::<WireCurrencyPatch>(json!({"value": "NOPE"}));
        assert!(error.contains("NOPE"));
        assert!(error.contains("unsupported code"));
        Ok(())
    }

    #[test]
    fn should_decode_and_round_trip_code_sets() -> Result<(), serde_json::Error> {
        let empty = serde_json::from_str::<WireShopTypes>("{}")?;
        assert!(empty.value.is_empty());

        let values =
            serde_json::from_str::<WireShopTypes>(r#"{"value":["AUCTION_HOUSE","MARKETPLACE"]}"#)?;
        assert_eq!(
            HashSet::from([ShopType::AuctionHouse, ShopType::Marketplace]),
            values.value
        );
        let serialized = serde_json::to_string(&values)?;
        let round_tripped: WireShopTypes = serde_json::from_str(&serialized)?;
        assert_eq!(values.value, round_tripped.value);

        let error = error_text::<WireShopTypes>(json!({"value": ["NOPE"]}));
        assert!(error.contains("NOPE"));
        assert!(error.contains("unsupported code"));
        Ok(())
    }

    #[test]
    fn should_decode_patch_code_sets_for_all_patch_states() -> Result<(), serde_json::Error> {
        assert_eq!(
            PatchValue::Omitted,
            serde_json::from_str::<WireShopTypePatchSet>("{}")?.value
        );
        assert_eq!(
            PatchValue::Null,
            serde_json::from_str::<WireShopTypePatchSet>(r#"{"value":null}"#)?.value
        );
        assert_eq!(
            PatchValue::Value(HashSet::new()),
            serde_json::from_str::<WireShopTypePatchSet>(r#"{"value":[]}"#)?.value
        );
        assert_eq!(
            PatchValue::Value(HashSet::from([ShopType::Marketplace])),
            serde_json::from_str::<WireShopTypePatchSet>(r#"{"value":["MARKETPLACE"]}"#,)?.value
        );

        let error = error_text::<WireShopTypePatchSet>(json!({"value": ["NOPE"]}));
        assert!(error.contains("NOPE"));
        assert!(error.contains("unsupported code"));
        Ok(())
    }

    #[test]
    fn should_serialize_rest_product_event_type_codes() -> Result<(), serde_json::Error> {
        let values = [
            (
                product_listing_service::use_cases::ProductListingEventType::Created,
                "CREATED",
            ),
            (
                product_listing_service::use_cases::ProductListingEventType::StateChanged,
                "STATE_CHANGED",
            ),
            (
                product_listing_service::use_cases::ProductListingEventType::AddressChanged,
                "ADDRESS_CHANGED",
            ),
            (
                product_listing_service::use_cases::ProductListingEventType::PriceChanged,
                "PRICE_CHANGED",
            ),
            (
                product_listing_service::use_cases::ProductListingEventType::UrlChanged,
                "URL_CHANGED",
            ),
            (
                product_listing_service::use_cases::ProductListingEventType::ImagesChanged,
                "IMAGES_CHANGED",
            ),
            (
                product_listing_service::use_cases::ProductListingEventType::AuctionChanged,
                "AUCTION_CHANGED",
            ),
            (
                product_listing_service::use_cases::ProductListingEventType::Deleted,
                "DELETED",
            ),
        ];

        for (value, expected) in values {
            assert_eq!(
                json!(expected),
                serde_json::to_value(WireProductEventType(value))?
            );
        }
        Ok(())
    }

    #[test]
    fn should_decode_code_sets_from_real_query_syntax() -> Result<(), serde_qs::Error> {
        let query: WireShopQuery =
            serde_qs::from_str("shopType[0]=AUCTION_HOUSE&shopType[1]=MARKETPLACE")?;
        assert_eq!(
            HashSet::from([ShopType::AuctionHouse, ShopType::Marketplace]),
            query.shop_types
        );
        Ok(())
    }

    fn error_text<T: DeserializeOwned>(value: serde_json::Value) -> String {
        match serde_json::from_value::<T>(value) {
            Ok(_) => "unexpected successful deserialization".to_owned(),
            Err(error) => error.to_string(),
        }
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
    #[serde(transparent)]
    struct WireProductEventType(
        #[serde(with = "product_event_type")]
        product_listing_service::use_cases::ProductListingEventType,
    );

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WireCurrencyOption {
        #[serde(default, with = "currency::option")]
        value: Option<Currency>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WireLanguageOption {
        #[serde(default, with = "language::option")]
        value: Option<Language>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WireCurrencyPatch {
        #[serde(default, deserialize_with = "currency::patch::deserialize")]
        value: PatchValue<Currency>,
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WireShopTypes {
        #[serde(default, with = "shop_type::set")]
        value: HashSet<ShopType>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WireShopTypePatchSet {
        #[serde(default, deserialize_with = "shop_type::patch_set::deserialize")]
        value: PatchValue<HashSet<ShopType>>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct WireShopQuery {
        #[serde(rename = "shopType", default, with = "shop_type::set")]
        shop_types: HashSet<ShopType>,
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ShopCodes {
        #[serde(with = "shop_type")]
        shop_type: ShopType,
        #[serde(with = "shop_partner_status")]
        partner_status: ShopPartnerStatus,
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CompatibilityCodes {
        #[serde(with = "product_state")]
        product_state: ProductState,
        #[serde(with = "product_lifecycle")]
        product_lifecycle: ProductLifecycle,
        #[serde(with = "distance_unit")]
        distance_unit: DistanceUnit,
        #[serde(with = "measurement_unit::option")]
        measurement_unit: Option<MeasurementUnit>,
        #[serde(with = "prohibited_content")]
        prohibited_content: ProhibitedContent,
    }
}
