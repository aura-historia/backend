use crate::actor::domain::{Actor, InvalidActorError};
use crate::actor::record::ActorRecord;
use crate::user_id::UserId;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(into = "String", try_from = "String")]
pub enum ActorData {
    User(UserId),
    System,
}

impl From<Actor> for ActorData {
    fn from(actor: Actor) -> Self {
        match actor {
            Actor::User(user_id) => Self::User(user_id),
            Actor::System => Self::System,
        }
    }
}

impl From<ActorData> for Actor {
    fn from(actor: ActorData) -> Self {
        match actor {
            ActorData::User(user_id) => Self::User(user_id),
            ActorData::System => Self::System,
        }
    }
}

impl From<ActorRecord> for ActorData {
    fn from(actor: ActorRecord) -> Self {
        Actor::from(actor).into()
    }
}

impl TryFrom<String> for ActorData {
    type Error = InvalidActorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Actor::try_from(value)?.into())
    }
}

impl From<ActorData> for String {
    fn from(actor: ActorData) -> Self {
        String::from(Actor::from(actor))
    }
}

#[cfg(test)]
mod tests {
    use super::ActorData;
    use crate::user_id::UserId;
    use rstest::rstest;

    #[rstest]
    #[case(ActorData::System, "\"SYSTEM\"")]
    #[case(ActorData::User(UserId::from(uuid::uuid!("00000000-0000-4000-8000-000000000001"))), "\"00000000-0000-4000-8000-000000000001\"")]
    fn should_serialize_actor_as_plain_string(#[case] actor: ActorData, #[case] expected: &str) {
        assert_eq!(serde_json::to_string(&actor).unwrap(), expected);
    }

    #[rstest]
    #[case("\"SYSTEM\"", ActorData::System)]
    #[case("\"00000000-0000-4000-8000-000000000001\"", ActorData::User(UserId::from(uuid::uuid!("00000000-0000-4000-8000-000000000001"))))]
    fn should_deserialize_actor_from_plain_string(
        #[case] actor: &str,
        #[case] expected: ActorData,
    ) {
        assert_eq!(serde_json::from_str::<ActorData>(actor).unwrap(), expected);
    }
}
