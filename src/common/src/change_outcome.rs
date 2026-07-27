#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOutcome {
    Changed,
    Unchanged,
}

impl ChangeOutcome {
    pub fn changed(self) -> bool {
        matches!(self, Self::Changed)
    }

    pub fn combine(self, other: Self) -> Self {
        if self.changed() || other.changed() {
            Self::Changed
        } else {
            Self::Unchanged
        }
    }
}

impl From<bool> for ChangeOutcome {
    fn from(changed: bool) -> Self {
        if changed {
            Self::Changed
        } else {
            Self::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_combine_to_changed_when_any_outcome_changed() {
        assert_eq!(
            ChangeOutcome::Changed,
            ChangeOutcome::Unchanged.combine(ChangeOutcome::Changed)
        );
    }

    #[test]
    fn should_combine_to_unchanged_when_both_outcomes_unchanged() {
        assert_eq!(
            ChangeOutcome::Unchanged,
            ChangeOutcome::Unchanged.combine(ChangeOutcome::Unchanged)
        );
    }
}
