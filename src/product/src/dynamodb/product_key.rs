use common::product_id::ProductKey;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;

/// Encode the legacy product key payload without the DynamoDB partition prefix.
///
/// This format belongs to the legacy DynamoDB boundary, not to `product-core`.
pub(crate) fn encode(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub(crate) fn decode(value: &str) -> Result<ProductKey, String> {
    if let Some((shop_id, shops_product_id)) = value
        .trim_start_matches("shop_id#")
        .split_once("#shops_product_id#")
    {
        Ok(ProductKey {
            shop_id: shop_id
                .try_into()
                .map_err(|error: uuid::Error| error.to_string())?,
            shops_product_id: shops_product_id.into(),
        })
    } else {
        Err(format!("Parsing ProductKey '{value}' failed."))
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;

    #[rstest::rstest]
    #[case::plain("123456")]
    #[case::with_separator("1874874-489746152-49874651-845")]
    fn should_round_trip_legacy_product_key(#[case] shops_product_id: &str) {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::from(shops_product_id);

        let encoded = encode(&shop_id, &shops_product_id);

        assert_eq!(
            format!("shop_id#{shop_id}#shops_product_id#{shops_product_id}"),
            encoded
        );
        assert_eq!(
            common::product_id::ProductKey::new(shop_id, shops_product_id),
            decode(&encoded).unwrap()
        );
    }
}
