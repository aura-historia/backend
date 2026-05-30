use crate::actor::data::ActorData;
use crate::actor::domain::{Actor, InvalidActorError};
use crate::user_id::UserId;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(into = "String", try_from = "String")]
pub enum ActorRecord {
    User(UserId),
    System,
}

impl From<Actor> for ActorRecord {
    fn from(actor: Actor) -> Self {
        match actor {
            Actor::User(user_id) => Self::User(user_id),
            Actor::System => Self::System,
        }
    }
}

impl From<ActorRecord> for Actor {
    fn from(actor: ActorRecord) -> Self {
        match actor {
            ActorRecord::User(user_id) => Self::User(user_id),
            ActorRecord::System => Self::System,
        }
    }
}

impl From<ActorData> for ActorRecord {
    fn from(actor: ActorData) -> Self {
        Actor::from(actor).into()
    }
}

impl TryFrom<String> for ActorRecord {
    type Error = InvalidActorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Actor::try_from(value)?.into())
    }
}

impl From<ActorRecord> for String {
    fn from(actor: ActorRecord) -> Self {
        String::from(Actor::from(actor))
    }
}

#[cfg(test)]
mod tests {
    use super::ActorRecord;
    use crate::user_id::UserId;
    use rstest::rstest;

    #[rstest]
    #[case(ActorRecord::System, "\"SYSTEM\"")]
    #[case(ActorRecord::User(UserId::from(uuid::uuid!("00000000-0000-4000-8000-000000000001"))), "\"00000000-0000-4000-8000-000000000001\"")]
    fn should_serialize_actor_as_plain_string(
        #[case] actor: ActorRecord,
        #[case] expected: &str,
    ) {
        assert_eq!(serde_json::to_string(&actor).unwrap(), expected);
    }

    #[rstest]
    #[case("\"SYSTEM\"", ActorRecord::System)]
    #[case("\"00000000-0000-4000-8000-000000000001\"", ActorRecord::User(UserId::from(uuid::uuid!("00000000-0000-4000-8000-000000000001"))))]
    fn should_deserialize_actor_from_plain_string(
        #[case] actor: &str,
        #[case] expected: ActorRecord,
    ) {
        assert_eq!(serde_json::from_str::<ActorRecord>(actor).unwrap(), expected);
    }
}
