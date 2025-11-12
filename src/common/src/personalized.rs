#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Personalized<Item, UserState> {
    pub item: Item,
    pub user_state: Option<UserState>,
}

#[cfg(feature = "api")]
pub mod api {
    use crate::personalized::Personalized;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PersonalizedData<ProductData, UserStateData> {
        pub item: ProductData,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub user_state: Option<UserStateData>,
    }

    impl<Item, UserState, ProductData, UserStateData> From<Personalized<Item, UserState>>
        for PersonalizedData<ProductData, UserStateData>
    where
        Item: Into<ProductData>,
        UserState: Into<UserStateData>,
    {
        fn from(personalized: Personalized<Item, UserState>) -> Self {
            PersonalizedData {
                item: personalized.item.into(),
                user_state: personalized.user_state.map(Into::into),
            }
        }
    }
}
