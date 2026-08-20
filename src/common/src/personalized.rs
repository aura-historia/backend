// Legacy shim. Owner: application. Remove after legacy common consumers migrate.
pub use application::personalized::Personalized;

#[cfg(feature = "api")]
pub mod api {
    use crate::personalized::Personalized;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PersonalizedData<ItemData, UserStateData> {
        pub item: ItemData,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub user_state: Option<UserStateData>,
    }

    impl<Item, UserState, ItemData, UserStateData> From<Personalized<Item, UserState>>
        for PersonalizedData<ItemData, UserStateData>
    where
        Item: Into<ItemData>,
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
