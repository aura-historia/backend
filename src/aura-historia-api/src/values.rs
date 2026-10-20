pub(crate) use aura_historia_contracts::values::{
    LocalizedTextData, PriceData, ProductListingPriceData,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
    use application::patch_field::PatchField;
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize)]
    struct PricePatch {
        #[serde(default)]
        value: PatchValue<PriceData>,
    }

    #[test]
    fn should_consume_shared_value_and_patch_fixture_in_rest() -> Result<(), serde_json::Error> {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../aura-historia-contracts/tests/fixtures/common-values.json"
        ))?;
        let price: PriceData = serde_json::from_value(fixture["price"].clone())?;
        assert_eq!(fixture["price"], serde_json::to_value(price)?);
        let localized: LocalizedTextData =
            serde_json::from_value(fixture["localizedTextInput"].clone())?;
        assert_eq!(
            fixture["localizedTextOutput"],
            serde_json::to_value(localized)?
        );
        for value in fixture["listingPrices"].as_array().expect("listing prices") {
            let price: ProductListingPriceData = serde_json::from_value(value.clone())?;
            assert_eq!(value, &serde_json::to_value(price)?);
        }
        for (name, expected) in [
            ("omitted", PatchField::Unchanged),
            ("null", PatchField::Clear),
            ("value", PatchField::Set(price)),
        ] {
            let patch: PricePatch = serde_json::from_value(fixture["patches"][name].clone())?;
            assert_eq!(expected, clearable(patch.value));
        }
        let null: PricePatch = serde_json::from_value(fixture["patches"]["null"].clone())?;
        let error = non_nullable_patch(null.value, "value").expect_err("REST rejects null");
        assert_eq!(crate::error::BAD_BODY_VALUE, error.code());
        Ok(())
    }
}
