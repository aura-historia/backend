use crate::operation_context::{AuthenticationRequired, OperationContext, Principal};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteMetadata {
    actor: String,
}

impl WriteMetadata {
    pub fn actor(&self) -> &str {
        &self.actor
    }
}

impl TryFrom<&OperationContext> for WriteMetadata {
    type Error = AuthenticationRequired;

    fn try_from(context: &OperationContext) -> Result<Self, Self::Error> {
        Self::try_from(&context.principal)
    }
}

impl TryFrom<&Principal> for WriteMetadata {
    type Error = AuthenticationRequired;

    fn try_from(principal: &Principal) -> Result<Self, Self::Error> {
        let actor = match principal {
            Principal::Anonymous => return Err(AuthenticationRequired),
            Principal::User(user_id) => user_id.to_string(),
            Principal::Service(service_id) => service_id.clone(),
            Principal::System => "SYSTEM".to_owned(),
        };

        Ok(Self { actor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        operation_context::{CorrelationId, RequestId},
        user_id::UserId,
    };

    #[test]
    fn should_create_write_metadata_for_system_principal() {
        let metadata = WriteMetadata::try_from(&Principal::System);

        assert!(matches!(metadata, Ok(ref value) if value.actor() == "SYSTEM"));
    }

    #[test]
    fn should_reject_anonymous_principal() {
        let metadata = WriteMetadata::try_from(&Principal::Anonymous);

        assert_eq!(Err(AuthenticationRequired), metadata);
    }

    #[test]
    fn should_create_write_metadata_from_context() {
        let user_id = UserId::new();
        let context = OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let metadata = WriteMetadata::try_from(&context);

        assert!(matches!(metadata, Ok(ref value) if value.actor() == user_id.to_string()));
    }
}
