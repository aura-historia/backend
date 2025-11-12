#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Personalized<Product, UserState> {
    pub item: Product,
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

    impl<Product, UserState, ProductData, UserStateData> From<Personalized<Product, UserState>>
        for PersonalizedData<ProductData, UserStateData>
    where
        Product: Into<ProductData>,
        UserState: Into<UserStateData>,
    {
        fn from(personalized: Personalized<Product, UserState>) -> Self {
            PersonalizedData {
                item: personalized.item.into(),
                user_state: personalized.user_state.map(Into::into),
            }
        }
    }
}
