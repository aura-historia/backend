#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PatchField<T> {
    #[default]
    Unchanged,
    Set(T),
    Clear,
}

impl<T> PatchField<T> {
    pub fn is_changed(&self) -> bool {
        !matches!(self, Self::Unchanged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_report_unchanged_when_field_is_unchanged() {
        assert!(!PatchField::<String>::Unchanged.is_changed());
    }

    #[test]
    fn should_report_changed_when_field_is_set() {
        assert!(PatchField::Set("value".to_owned()).is_changed());
    }

    #[test]
    fn should_report_changed_when_field_is_clear() {
        assert!(PatchField::<String>::Clear.is_changed());
    }
}
