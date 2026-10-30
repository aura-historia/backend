use crate::ports::{
    UserAdminReadError, UserAdminReaderFactory, UserSearchReadError, UserSearchReader,
    UserSearchReaderFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use application::pagination::{Cursor, KeysetCursor, ScopeBinding, SortKey};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::sort::{Sort, SortOrder};
use serde_email::Email;
use time::OffsetDateTime;
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user_id::UserId;
use user_core::{
    role::UserRole, sort_user_field::SortUserField, tier::UserTier, user_search::UserSearch,
};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersRequest {
    pub search: UserSearch,
    pub sort: Option<Sort<SortUserField>>,
    pub cursor: Option<Cursor<UserSearchCursor>>,
}

impl SearchUsersRequest {
    /// The requested sort, or name ascending.
    pub fn effective_sort(&self) -> Sort<SortUserField> {
        self.sort.unwrap_or(Sort {
            sort: SortUserField::default(),
            order: SortOrder::Asc,
        })
    }

    pub fn scope_binding(&self) -> ScopeBinding {
        let search = &self.search;
        let sort = self.effective_sort();
        ScopeBinding::builder("user-search/v1")
            .optional_part(search.query.as_deref())
            .optional_part(search.email_query.as_deref())
            .optional_part(search.first_name_query.as_deref())
            .optional_part(search.last_name_query.as_deref())
            .code_set(search.tier_query.iter().map(|tier| tier.as_str()))
            .code_set(search.role_query.iter().map(|role| role.as_str()))
            .timestamp_range(search.created)
            .timestamp_range(search.updated)
            .part(sort_field_code(sort.sort))
            .part(sort.order.as_str())
            .finish()
    }

    fn continuation_in_scope(&self) -> bool {
        self.cursor
            .as_ref()
            .and_then(|cursor| cursor.search_after.as_ref())
            .is_none_or(|after| after.continues(&self.scope_binding(), self.effective_sort().sort))
    }
}

fn sort_field_code(field: SortUserField) -> &'static str {
    match field {
        SortUserField::Name => "name",
        SortUserField::Email => "email",
        SortUserField::FirstName => "firstName",
        SortUserField::LastName => "lastName",
        SortUserField::Tier => "tier",
        SortUserField::Role => "role",
        SortUserField::Created => "created",
        SortUserField::Updated => "updated",
    }
}

pub type UserSearchCursor = KeysetCursor<UserSortKey, UserId>;

/// Stored values of the sorted fields; profile names may be absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserSortKey {
    Name {
        first_name: Option<String>,
        last_name: Option<String>,
    },
    Email(String),
    FirstName(Option<String>),
    LastName(Option<String>),
    Tier(UserTier),
    Role(UserRole),
    Created(OffsetDateTime),
    Updated(OffsetDateTime),
}

impl SortKey for UserSortKey {
    type Field = SortUserField;

    fn field(&self) -> SortUserField {
        match self {
            Self::Name { .. } => SortUserField::Name,
            Self::Email(_) => SortUserField::Email,
            Self::FirstName(_) => SortUserField::FirstName,
            Self::LastName(_) => SortUserField::LastName,
            Self::Tier(_) => SortUserField::Tier,
            Self::Role(_) => SortUserField::Role,
            Self::Created(_) => SortUserField::Created,
            Self::Updated(_) => SortUserField::Updated,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserSummary {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<user_core::first_name::FirstName>,
    pub last_name: Option<user_core::last_name::LastName>,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersResult {
    pub items: Vec<UserSummary>,
    pub cursor: Cursor<UserSearchCursor>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchUsersError {
    #[error("authenticated actor required to search users")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user search cursor belongs to another filter or sort scope")]
    CursorScopeMismatch,
    #[error("temporary user search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search users transaction")]
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit search users transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
}

#[async_trait::async_trait]
pub trait SearchUsersUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError>;
}

pub struct SearchUsersHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admin_reader: A,
}

impl<U, R, A> SearchUsersHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> SearchUsersUseCase for SearchUsersHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserSearchReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "search_users",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError> {
        require_admin_actor_credential(context, CredentialCapability::UsersRead)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        if !request.continuation_in_scope() {
            return Err(SearchUsersError::CursorScopeMismatch);
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| SearchUsersError::BeginTransactionFailed(Box::new(source)))?;
        authorize_search_users(context, &mut tx, &self.admin_reader).await?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|source| SearchUsersError::CommitTransactionFailed(Box::new(source)))?;

        Ok(result)
    }
}

async fn authorize_search_users<Tx, A>(
    context: &OperationContext,
    tx: &mut Tx,
    admin_reader: &A,
) -> Result<(), SearchUsersError>
where
    Tx: Transaction,
    A: UserAdminReaderFactory<Tx>,
{
    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            let mut reader = admin_reader.in_transaction(tx);
            require_admin_actor(context, &mut reader)
                .await
                .map_err(SearchUsersError::from)
        }
        Principal::Anonymous => Err(SearchUsersError::AuthenticatedActorRequired),
    }
}

impl From<RequireAdminActorError> for SearchUsersError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for SearchUsersError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<UserSearchReadError> for SearchUsersError {
    fn from(error: UserSearchReadError) -> Self {
        match error {
            UserSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserSearchReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserSearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{UserAdminActorView, UserAdminReader};
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<usize>>);

    struct FakeTx;

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fakes {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            *self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
            Ok(FakeTx)
        }
    }

    impl UserAdminReaderFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            self.clone()
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for Fakes {
        async fn find_admin_actor(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(Some(UserAdminActorView {
                user_id,
                role: UserRole::Admin,
            }))
        }
    }

    impl UserSearchReaderFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserSearchReader + 'tx {
            self.clone()
        }
    }

    #[async_trait::async_trait]
    impl UserSearchReader for Fakes {
        async fn search(
            &mut self,
            _request: &SearchUsersRequest,
        ) -> Result<SearchUsersResult, UserSearchReadError> {
            Ok(SearchUsersResult {
                items: vec![],
                cursor: Cursor::default(),
                total: None,
            })
        }
    }

    fn request(sort: Option<Sort<SortUserField>>) -> SearchUsersRequest {
        SearchUsersRequest {
            search: UserSearch::default(),
            sort,
            cursor: None,
        }
    }

    fn continuation(request: &SearchUsersRequest, sort_key: UserSortKey) -> SearchUsersRequest {
        SearchUsersRequest {
            cursor: Some(Cursor {
                size: 2,
                search_after: Some(UserSearchCursor {
                    scope: request.scope_binding(),
                    sort_key,
                    id: UserId::new(),
                }),
            }),
            ..request.clone()
        }
    }

    #[test]
    fn should_bind_cursor_to_effective_filters_and_sort() {
        let name = UserSortKey::Name {
            first_name: Some("Ada".to_owned()),
            last_name: None,
        };
        let default = request(None);
        let tiers = SearchUsersRequest {
            search: UserSearch {
                tier_query: std::collections::HashSet::from([UserTier::Pro, UserTier::Free]).into(),
                ..UserSearch::default()
            },
            ..request(None)
        };
        let email_desc = request(Some(Sort {
            sort: SortUserField::Email,
            order: SortOrder::Desc,
        }));

        assert_eq!(
            default.scope_binding(),
            request(Some(Sort {
                sort: SortUserField::Name,
                order: SortOrder::Asc,
            }))
            .scope_binding()
        );
        assert!(continuation(&default, name.clone()).continuation_in_scope());
        assert!(continuation(&tiers, name.clone()).continuation_in_scope());
        assert!(
            !SearchUsersRequest {
                search: UserSearch::default(),
                ..continuation(&tiers, name.clone())
            }
            .continuation_in_scope()
        );
        assert!(
            !SearchUsersRequest {
                sort: email_desc.sort,
                ..continuation(&default, name)
            }
            .continuation_in_scope()
        );
        assert!(
            !continuation(&email_desc, UserSortKey::Tier(UserTier::Pro)).continuation_in_scope()
        );
    }

    #[tokio::test]
    async fn should_reject_cross_scope_cursor_before_beginning_user_search_transaction() {
        let fakes = Fakes::default();
        let handler = SearchUsersHandler::new(fakes.clone(), fakes.clone(), fakes.clone());
        let context = OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        };
        let foreign = SearchUsersRequest {
            sort: Some(Sort {
                sort: SortUserField::Created,
                order: SortOrder::Desc,
            }),
            ..continuation(
                &request(None),
                UserSortKey::Email("a@example.test".to_owned()),
            )
        };

        let rejected = handler.execute(&context, foreign).await;
        let accepted = handler
            .execute(
                &context,
                continuation(
                    &request(None),
                    UserSortKey::Name {
                        first_name: None,
                        last_name: None,
                    },
                ),
            )
            .await;

        assert!(matches!(
            rejected,
            Err(SearchUsersError::CursorScopeMismatch)
        ));
        assert!(accepted.is_ok());
        assert_eq!(
            1,
            *fakes
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
    }
}
