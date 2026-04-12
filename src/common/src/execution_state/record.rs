use crate::execution_state::domain::ExecutionState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionStateRecord {
    Processing,
    Waiting,
    Completed,
}

impl From<ExecutionState> for ExecutionStateRecord {
    fn from(state: ExecutionState) -> Self {
        match state {
            ExecutionState::Processing => ExecutionStateRecord::Processing,
            ExecutionState::Waiting => ExecutionStateRecord::Waiting,
            ExecutionState::Completed => ExecutionStateRecord::Completed,
        }
    }
}

impl From<ExecutionStateRecord> for ExecutionState {
    fn from(record: ExecutionStateRecord) -> Self {
        match record {
            ExecutionStateRecord::Processing => ExecutionState::Processing,
            ExecutionStateRecord::Waiting => ExecutionState::Waiting,
            ExecutionStateRecord::Completed => ExecutionState::Completed,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ExecutionStateRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let index: u8 = config.fake_with_rng(rng);
            match index % 3 {
                0 => ExecutionStateRecord::Processing,
                1 => ExecutionStateRecord::Waiting,
                _ => ExecutionStateRecord::Completed,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_convert_execution_state_domain_to_record_and_back() {
        let states = [
            ExecutionState::Processing,
            ExecutionState::Waiting,
            ExecutionState::Completed,
        ];
        for state in states {
            let record: ExecutionStateRecord = state.into();
            let converted: ExecutionState = record.into();
            assert_eq!(state, converted);
        }
    }
}
