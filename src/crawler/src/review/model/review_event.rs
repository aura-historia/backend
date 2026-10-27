use serde::{Serialize, Serializer};

/// Who made a review mutation. The review console resolves it from the request's credential:
/// the agent token acts as `Agent`; the review token, a browser session and the `dev` binary's
/// local same-origin requests act as `Human`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewActor {
    Human,
    Agent,
}

impl ReviewActor {
    pub const ALL: [Self; 2] = [Self::Human, Self::Agent];

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "HUMAN",
            Self::Agent => "AGENT",
        }
    }

    /// The actor a persisted name stands for, or `None` for a name no actor has.
    pub fn from_persisted(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|actor| actor.as_str() == value)
    }
}

impl std::fmt::Display for ReviewActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ReviewActor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_read_back_every_persisted_actor_and_refuse_any_other_name() {
        for actor in ReviewActor::ALL {
            assert_eq!(ReviewActor::from_persisted(actor.as_str()), Some(actor));
            assert_eq!(
                serde_json::to_value(actor).ok(),
                Some(serde_json::Value::from(actor.as_str()))
            );
        }
        for unknown in ["human", "Agent", "", "ROBOT"] {
            assert_eq!(ReviewActor::from_persisted(unknown), None, "{unknown}");
        }
    }
}
