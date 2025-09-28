use crate::shop_id::ShopId;
use crate::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct ItemKey {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
}

impl ItemKey {
    pub fn new(shop_id: ShopId, shops_item_id: ShopsItemId) -> Self {
        ItemKey {
            shop_id,
            shops_item_id,
        }
    }
}

impl From<ItemKey> for String {
    fn from(key: ItemKey) -> Self {
        format!(
            "shop_id#{}#shops_item_id#{}",
            key.shop_id, key.shops_item_id
        )
    }
}

impl Display for ItemKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "shop_id#{}#shops_item_id#{}",
            self.shop_id, self.shops_item_id
        )
    }
}

impl TryFrom<&str> for ItemKey {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if let Some((shop_id, shops_item_id)) = value
            .trim_start_matches("shop_id#")
            .split_once("#shops_item_id#")
        {
            Ok(ItemKey {
                shop_id: shop_id
                    .try_into()
                    .map_err(|err: uuid::Error| err.to_string())?,
                shops_item_id: shops_item_id.into(),
            })
        } else {
            Err(format!("Parsing ItemKey '{value}' failed."))
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{shop_id::ShopId, shops_item_id::ShopsItemId};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ItemKeyData {
        pub shop_id: ShopId,
        pub shops_item_id: ShopsItemId,
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ItemId(Uuid);

impl Default for ItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl ItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Display for ItemId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for ItemId {
    fn from(uuid: Uuid) -> Self {
        ItemId(uuid)
    }
}

impl TryFrom<String> for ItemId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<ItemId> for String {
    fn from(id: ItemId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for ItemId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    #[rstest::rstest]
    #[case::differing(uuid::Uuid::new_v4().to_string(), "123456")]
    #[case::item_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874#489746152")]
    #[case::item_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874#489746152#49874651#845")]
    fn should_display_item_key(#[case] shop_id: String, #[case] shops_item_id: &str) {
        use crate::item_id::ItemKey;

        let expected = format!("shop_id#{shop_id}#shops_item_id#{shops_item_id}");

        let actual = ItemKey {
            shop_id: shop_id.try_into().unwrap(),
            shops_item_id: shops_item_id.into(),
        }
        .to_string();

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case::differing(uuid::Uuid::new_v4().to_string(), "123456")]
    #[case::item_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874#489746152")]
    #[case::item_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874#489746152#49874651#845")]
    fn should_into_string_item_key(#[case] shop_id: String, #[case] shops_item_id: &str) {
        use crate::item_id::ItemKey;

        let expected = format!("shop_id#{shop_id}#shops_item_id#{shops_item_id}");

        let actual: String = ItemKey {
            shop_id: shop_id.try_into().unwrap(),
            shops_item_id: shops_item_id.into(),
        }
        .into();

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case::differing(uuid::Uuid::new_v4().to_string(), "123456")]
    #[case::item_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874#489746152")]
    #[case::item_containing_separator(uuid::Uuid::new_v4().to_string(), "1874874#489746152#49874651#845")]
    fn should_parse_item_key(#[case] shop_id: String, #[case] shops_item_id: &str) {
        use crate::item_id::ItemKey;

        let payload = format!("shop_id#{shop_id}#shops_item_id#{shops_item_id}");
        let actual = ItemKey::try_from(payload.as_str());

        let expected = ItemKey {
            shop_id: shop_id.try_into().unwrap(),
            shops_item_id: shops_item_id.into(),
        };

        assert_eq!(expected, actual.unwrap());
    }
}
