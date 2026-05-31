use crate::user_id::UserId;
use std::fmt::{Display, Formatter};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Actor {
    User(UserId),
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidActorError {
    #[error("Invalid actor '{0}'")]
    InvalidActor(String),
    #[error("Invalid actor user-id")]
    InvalidUserId(#[from] uuid::Error),
}

impl Actor {
    pub const SYSTEM: &str = "SYSTEM";
}

impl From<UserId> for Actor {
    fn from(user_id: UserId) -> Self {
        Self::User(user_id)
    }
}

impl TryFrom<&str> for Actor {
    type Error = InvalidActorError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value == Self::SYSTEM {
            Ok(Self::System)
        } else if value.is_empty() {
            Err(InvalidActorError::InvalidActor(value.to_owned()))
        } else {
            Ok(Self::User(UserId::try_from(value)?))
        }
    }
}

impl TryFrom<String> for Actor {
    type Error = InvalidActorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<Actor> for String {
    fn from(actor: Actor) -> Self {
        match actor {
            Actor::User(user_id) => user_id.to_string(),
            Actor::System => Actor::SYSTEM.to_owned(),
        }
    }
}

impl Display for Actor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(*self))
    }
}

#[cfg(test)]
mod tests {
    use super::Actor;
    use crate::user_id::UserId;
    use rstest::rstest;

    #[test]
    fn should_format_system_actor_as_plain_string() {
        assert_eq!(String::from(Actor::System), "SYSTEM");
    }

    #[test]
    fn should_format_user_actor_as_plain_string() {
        let user_id = UserId::new();
        assert_eq!(String::from(Actor::User(user_id)), user_id.to_string());
    }

    #[test]
    fn should_parse_system_actor() {
        assert_eq!(Actor::try_from("SYSTEM").unwrap(), Actor::System);
    }

    #[test]
    fn should_parse_user_actor() {
        let user_id = UserId::new();
        assert_eq!(
            Actor::try_from(user_id.to_string()).unwrap(),
            Actor::User(user_id)
        );
    }

    #[rstest]
    #[case("")]
    #[case("system")]
    #[case("not-a-uuid")]
    fn should_reject_invalid_actor(#[case] value: &str) {
        assert!(Actor::try_from(value).is_err());
    }
}
