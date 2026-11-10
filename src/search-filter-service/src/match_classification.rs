//! Classifier decision settings deployed for saved-search matching.

use classifier_model::Probability;
use std::num::NonZeroUsize;

const MAX_BASIS_POINTS: u16 = 10_000;
const MAX_BATCH_CANDIDATES: usize = 50;

/// Scheduled matching's inclusive `should_show` acceptance threshold and the largest number of
/// text candidates one classifier request may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchClassificationSettings {
    should_show_threshold: Probability,
    batch_candidates: NonZeroUsize,
}

#[derive(Debug, thiserror::Error)]
pub enum MatchClassificationSettingsError {
    #[error("should_show threshold exceeds {MAX_BASIS_POINTS} basis points")]
    ThresholdOutOfRange,
    #[error("classification batch size must be between 1 and {MAX_BATCH_CANDIDATES}")]
    BatchCandidatesOutOfRange,
}

/// The inclusive `should_show` acceptance threshold from its deployed basis points.
pub fn should_show_threshold(
    basis_points: u16,
) -> Result<Probability, MatchClassificationSettingsError> {
    if basis_points > MAX_BASIS_POINTS {
        return Err(MatchClassificationSettingsError::ThresholdOutOfRange);
    }
    Probability::new(f64::from(basis_points) / f64::from(MAX_BASIS_POINTS))
        .map_err(|_| MatchClassificationSettingsError::ThresholdOutOfRange)
}

impl MatchClassificationSettings {
    pub fn new(
        should_show_threshold_basis_points: u16,
        batch_candidates: usize,
    ) -> Result<Self, MatchClassificationSettingsError> {
        let should_show_threshold = should_show_threshold(should_show_threshold_basis_points)?;
        let batch_candidates = NonZeroUsize::new(batch_candidates)
            .filter(|value| value.get() <= MAX_BATCH_CANDIDATES)
            .ok_or(MatchClassificationSettingsError::BatchCandidatesOutOfRange)?;
        Ok(Self {
            should_show_threshold,
            batch_candidates,
        })
    }

    pub const fn should_show_threshold(&self) -> Probability {
        self.should_show_threshold
    }

    pub const fn batch_candidates(&self) -> NonZeroUsize {
        self.batch_candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_convert_basis_points_to_an_inclusive_probability() {
        let settings = MatchClassificationSettings::new(5_000, 1).unwrap();

        assert_eq!(0.5, settings.should_show_threshold().get());
        assert_eq!(1, settings.batch_candidates().get());
        assert_eq!(
            1.0,
            MatchClassificationSettings::new(10_000, 50)
                .unwrap()
                .should_show_threshold()
                .get()
        );
    }

    #[test]
    fn should_reject_settings_outside_the_shared_bounds() {
        assert!(matches!(
            MatchClassificationSettings::new(10_001, 1),
            Err(MatchClassificationSettingsError::ThresholdOutOfRange)
        ));
        for batch_candidates in [0, 51] {
            assert!(matches!(
                MatchClassificationSettings::new(5_000, batch_candidates),
                Err(MatchClassificationSettingsError::BatchCandidatesOutOfRange)
            ));
        }
    }
}
