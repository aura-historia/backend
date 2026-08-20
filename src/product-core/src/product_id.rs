use crate::shops_product_id::ShopsProductId;
use shop_core::shop_id::ShopId;
use std::fmt::{Display, Formatter};

domain_primitives::uuid_v4_newtype!(ProductId);

impl From<ProductId> for uuid::Uuid {
    fn from(id: ProductId) -> Self {
        id.0
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct ProductKey {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
}

impl ProductKey {
    pub fn new(shop_id: ShopId, shops_product_id: ShopsProductId) -> Self {
        Self {
            shop_id,
            shops_product_id,
        }
    }
}

impl From<ProductKey> for String {
    fn from(key: ProductKey) -> Self {
        key.to_string()
    }
}

impl Display for ProductKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "shop_id#{}#shops_product_id#{}",
            self.shop_id, self.shops_product_id
        )
    }
}

impl TryFrom<&str> for ProductKey {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if let Some((shop_id, shops_product_id)) = value
            .trim_start_matches("shop_id#")
            .split_once("#shops_product_id#")
        {
            Ok(Self {
                shop_id: shop_id
                    .try_into()
                    .map_err(|error: uuid::Error| error.to_string())?,
                shops_product_id: shops_product_id.into(),
            })
        } else {
            Err(format!("Parsing ProductKey '{value}' failed."))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case::plain(uuid::Uuid::new_v4().to_string(), "123456")]
    #[case::with_separator(uuid::Uuid::new_v4().to_string(), "1874874-489746152-49874651-845")]
    fn should_round_trip_product_key(#[case] shop_id: String, #[case] shops_product_id: &str) {
        let expected = ProductKey::new(shop_id.try_into().unwrap(), shops_product_id.into());

        let actual = ProductKey::try_from(expected.to_string().as_str()).unwrap();

        assert_eq!(expected, actual);
    }
}
