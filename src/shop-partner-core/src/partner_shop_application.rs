use crate::partner_shop_application_state::PartnerShopApplicationState;
use common::execution_state::domain::ExecutionState;
use common::{
    partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId, user_id::UserId,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopApplication {
    id: PartnerShopApplicationId,
    applicant_user_id: UserId,
    business_state: PartnerShopApplicationState,
    execution_state: ExecutionState,
    payload: PartnerShopApplicationPayload,
    task_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnerShopApplicationPayload {
    Existing { shop_id: ShopId },
    New { shop_id: ShopId },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewPartnerShopApplication {
    pub id: PartnerShopApplicationId,
    pub applicant_user_id: UserId,
    pub payload: PartnerShopApplicationPayload,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedPartnerShopApplicationState {
    pub id: PartnerShopApplicationId,
    pub applicant_user_id: UserId,
    pub business_state: PartnerShopApplicationState,
    pub execution_state: ExecutionState,
    pub payload: PartnerShopApplicationPayload,
    pub task_token: Option<String>,
}

impl PartnerShopApplication {
    pub fn create(input: NewPartnerShopApplication) -> Self {
        Self {
            id: input.id,
            applicant_user_id: input.applicant_user_id,
            business_state: PartnerShopApplicationState::Submitted,
            execution_state: ExecutionState::Processing,
            payload: input.payload,
            task_token: None,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartnerShopApplicationState) -> Self {
        Self {
            id: state.id,
            applicant_user_id: state.applicant_user_id,
            business_state: state.business_state,
            execution_state: state.execution_state,
            payload: state.payload,
            task_token: state.task_token,
        }
    }

    pub fn mark_in_review(&mut self, task_token: String) {
        self.business_state = PartnerShopApplicationState::InReview;
        self.execution_state = ExecutionState::Waiting;
        self.task_token = Some(task_token);
    }

    pub fn approve(&mut self) {
        self.business_state = PartnerShopApplicationState::Approved;
        self.execution_state = ExecutionState::Completed;
    }

    pub fn reject(&mut self) {
        self.business_state = PartnerShopApplicationState::Rejected;
        self.execution_state = ExecutionState::Completed;
    }

    pub fn id(&self) -> PartnerShopApplicationId {
        self.id
    }
    pub fn applicant_user_id(&self) -> UserId {
        self.applicant_user_id
    }
    pub fn business_state(&self) -> PartnerShopApplicationState {
        self.business_state
    }
    pub fn execution_state(&self) -> ExecutionState {
        self.execution_state
    }
    pub fn payload(&self) -> PartnerShopApplicationPayload {
        self.payload
    }
    pub fn shop_id(&self) -> ShopId {
        match self.payload {
            PartnerShopApplicationPayload::Existing { shop_id }
            | PartnerShopApplicationPayload::New { shop_id } => shop_id,
        }
    }
    pub fn task_token(&self) -> Option<&str> {
        self.task_token.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_submitted_application_linked_to_new_shop() {
        let shop_id = ShopId::new();
        let app = PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
            payload: PartnerShopApplicationPayload::New { shop_id },
        });

        assert_eq!(shop_id, app.shop_id());
        assert_eq!(
            PartnerShopApplicationPayload::New { shop_id },
            app.payload()
        );
        assert_eq!(PartnerShopApplicationState::Submitted, app.business_state());
        assert_eq!(ExecutionState::Processing, app.execution_state());
        assert_eq!(None, app.task_token());
    }

    #[test]
    fn should_create_submitted_application_linked_to_existing_shop() {
        let shop_id = ShopId::new();
        let app = PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
            payload: PartnerShopApplicationPayload::Existing { shop_id },
        });

        assert_eq!(shop_id, app.shop_id());
        assert_eq!(
            PartnerShopApplicationPayload::Existing { shop_id },
            app.payload()
        );
    }

    #[test]
    fn should_rehydrate_application_state() {
        let id = PartnerShopApplicationId::new();
        let user_id = UserId::new();
        let shop_id = ShopId::new();
        let app = PartnerShopApplication::rehydrate(RehydratedPartnerShopApplicationState {
            id,
            applicant_user_id: user_id,
            business_state: PartnerShopApplicationState::InReview,
            execution_state: ExecutionState::Waiting,
            payload: PartnerShopApplicationPayload::Existing { shop_id },
            task_token: Some("token".to_owned()),
        });

        assert_eq!(id, app.id());
        assert_eq!(user_id, app.applicant_user_id());
        assert_eq!(Some("token"), app.task_token());
    }

    #[test]
    fn should_mark_in_review_with_token() {
        let mut app = application();
        app.mark_in_review("task-token".to_owned());
        assert_eq!(PartnerShopApplicationState::InReview, app.business_state());
        assert_eq!(ExecutionState::Waiting, app.execution_state());
        assert_eq!(Some("task-token"), app.task_token());
    }

    #[test]
    fn should_approve_application() {
        let mut app = application();
        app.approve();
        assert_eq!(PartnerShopApplicationState::Approved, app.business_state());
        assert_eq!(ExecutionState::Completed, app.execution_state());
    }

    #[test]
    fn should_reject_application() {
        let mut app = application();
        app.reject();
        assert_eq!(PartnerShopApplicationState::Rejected, app.business_state());
        assert_eq!(ExecutionState::Completed, app.execution_state());
    }

    fn application() -> PartnerShopApplication {
        PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
            payload: PartnerShopApplicationPayload::New {
                shop_id: ShopId::new(),
            },
        })
    }
}
