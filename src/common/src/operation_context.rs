// Legacy shim. Owner: application. Remove after legacy callers migrate.
pub use application::operation_context::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_id::UserId;
    use std::collections::BTreeSet;

    #[test]
    fn should_preserve_legacy_operation_context_paths() {
        let context = OperationContext {
            principal: Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::from([CredentialCapability::ProductsWrite]),
            },
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        };

        assert!(context.principal.is_authenticated());
    }
}
