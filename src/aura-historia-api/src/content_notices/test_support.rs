//! Stand-ins for the content notice use cases a router test does not exercise.

use application::operation_context::OperationContext;
use content_notice_service::use_cases::{
    commands::submit_content_notice::{
        SubmitContentNoticeCommand, SubmitContentNoticeError, SubmitContentNoticeResult,
        SubmitContentNoticeUseCase,
    },
    queries::{
        get_admin_content_notice::{
            AdminContentNoticeDetails, GetAdminContentNoticeError, GetAdminContentNoticeRequest,
            GetAdminContentNoticeUseCase,
        },
        list_admin_content_notices::{
            ListAdminContentNoticesError, ListAdminContentNoticesRequest,
            ListAdminContentNoticesResult, ListAdminContentNoticesUseCase,
        },
    },
};

/// Shares the router with the use case under test and fails the test when reached.
pub(super) struct Unused;

#[async_trait::async_trait]
impl SubmitContentNoticeUseCase for Unused {
    async fn execute(
        &self,
        _: &OperationContext,
        _: SubmitContentNoticeCommand,
    ) -> Result<SubmitContentNoticeResult, SubmitContentNoticeError> {
        unreachable!("this test does not submit notices")
    }
}

#[async_trait::async_trait]
impl ListAdminContentNoticesUseCase for Unused {
    async fn execute(
        &self,
        _: &OperationContext,
        _: ListAdminContentNoticesRequest,
    ) -> Result<ListAdminContentNoticesResult, ListAdminContentNoticesError> {
        unreachable!("this test does not list notices")
    }
}

#[async_trait::async_trait]
impl GetAdminContentNoticeUseCase for Unused {
    async fn execute(
        &self,
        _: &OperationContext,
        _: GetAdminContentNoticeRequest,
    ) -> Result<AdminContentNoticeDetails, GetAdminContentNoticeError> {
        unreachable!("this test does not read notice details")
    }
}
