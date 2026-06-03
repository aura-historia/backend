use crate::actor::data::ActorData;
use crate::actor::domain::{Actor, InvalidActorError};
use crate::actor::record::ActorRecord;
use crate::user_id::UserId;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(into = "String", try_from = "String")]
pub enum ActorDocument {
    User(UserId),
    System,
}

impl From<Actor> for ActorDocument {
    fn from(actor: Actor) -> Self {
        match actor {
            Actor::User(user_id) => Self::User(user_id),
            Actor::System => Self::System,
        }
    }
}

impl From<ActorData> for ActorDocument {
    fn from(actor: ActorData) -> Self {
        Actor::from(actor).into()
    }
}

impl From<ActorRecord> for ActorDocument {
    fn from(actor: ActorRecord) -> Self {
        Actor::from(actor).into()
    }
}

impl From<ActorDocument> for Actor {
    fn from(actor: ActorDocument) -> Self {
        match actor {
            ActorDocument::User(user_id) => Self::User(user_id),
            ActorDocument::System => Self::System,
        }
    }
}

impl TryFrom<String> for ActorDocument {
    type Error = InvalidActorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Actor::try_from(value)?.into())
    }
}

impl From<ActorDocument> for String {
    fn from(actor: ActorDocument) -> Self {
        String::from(Actor::from(actor))
    }
}

#[cfg(test)]
mod tests {
    use super::ActorDocument;
    use crate::user_id::UserId;
    use rstest::rstest;

    #[rstest]
    #[case(ActorDocument::System, "\"SYSTEM\"")]
    #[case(ActorDocument::User(UserId::from(uuid::uuid!("00000000-0000-4000-8000-000000000001"))), "\"00000000-0000-4000-8000-000000000001\"")]
    fn should_serialize_actor_as_plain_string(
        #[case] actor: ActorDocument,
        #[case] expected: &str,
    ) {
        assert_eq!(serde_json::to_string(&actor).unwrap(), expected);
    }

    #[rstest]
    #[case("\"SYSTEM\"", ActorDocument::System)]
    #[case("\"00000000-0000-4000-8000-000000000001\"", ActorDocument::User(UserId::from(uuid::uuid!("00000000-0000-4000-8000-000000000001"))))]
    fn should_deserialize_actor_from_plain_string(
        #[case] actor: &str,
        #[case] expected: ActorDocument,
    ) {
        assert_eq!(
            serde_json::from_str::<ActorDocument>(actor).unwrap(),
            expected
        );
    }
}
