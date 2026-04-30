#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UpdateUserSearchFilterMatchCommand {
    pub feedback: Option<bool>,
}

impl UpdateUserSearchFilterMatchCommand {
    pub fn is_empty(&self) -> bool {
        self.feedback.is_none()
    }
}
