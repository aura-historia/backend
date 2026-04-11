use common::execution_state::ExecutionState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionStateData {
    Processing,
    Waiting,
    Completed,
}

impl From<ExecutionState> for ExecutionStateData {
    fn from(state: ExecutionState) -> Self {
        match state {
            ExecutionState::Processing => ExecutionStateData::Processing,
            ExecutionState::Waiting => ExecutionStateData::Waiting,
            ExecutionState::Completed => ExecutionStateData::Completed,
        }
    }
}

impl From<ExecutionStateData> for ExecutionState {
    fn from(data: ExecutionStateData) -> Self {
        match data {
            ExecutionStateData::Processing => ExecutionState::Processing,
            ExecutionStateData::Waiting => ExecutionState::Waiting,
            ExecutionStateData::Completed => ExecutionState::Completed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_convert_execution_state_domain_to_data_and_back() {
        let states = [
            ExecutionState::Processing,
            ExecutionState::Waiting,
            ExecutionState::Completed,
        ];
        for state in states {
            let data: ExecutionStateData = state.into();
            let converted: ExecutionState = data.into();
            assert_eq!(state, converted);
        }
    }
}
