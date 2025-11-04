#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Personalized<Item, UserState> {
    pub item: Item,
    pub user_state: Option<UserState>,
}
