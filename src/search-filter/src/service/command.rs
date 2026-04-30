#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UpdateUserSearchFilterMatchCommand {
    pub matches_feedback: Option<bool>,
}

impl UpdateUserSearchFilterMatchCommand {
    pub fn is_empty(&self) -> bool {
        self.matches_feedback.is_none()
    }
}
