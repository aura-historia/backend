use crate::partner_shop_application_state::PartnerShopApplicationState;
use common::{
    partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId, user_id::UserId,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopApplication {
    id: PartnerShopApplicationId,
    applicant_user_id: UserId,
    business_state: PartnerShopApplicationState,
    payload: PartnerShopApplicationPayload,
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
    pub payload: PartnerShopApplicationPayload,
}

impl PartnerShopApplication {
    pub fn create(input: NewPartnerShopApplication) -> Self {
        Self {
            id: input.id,
            applicant_user_id: input.applicant_user_id,
            business_state: PartnerShopApplicationState::Submitted,
            payload: input.payload,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartnerShopApplicationState) -> Self {
        Self {
            id: state.id,
            applicant_user_id: state.applicant_user_id,
            business_state: state.business_state,
            payload: state.payload,
        }
    }

    pub fn mark_in_review(&mut self) -> Result<(), PartnerShopApplicationTransitionError> {
        self.transition(PartnerShopApplicationState::InReview)
    }

    pub fn approve(&mut self) -> Result<(), PartnerShopApplicationTransitionError> {
        self.transition(PartnerShopApplicationState::Approved)
    }

    pub fn reject(&mut self) -> Result<(), PartnerShopApplicationTransitionError> {
        self.transition(PartnerShopApplicationState::Rejected)
    }

    pub fn withdraw(&mut self) -> Result<(), PartnerShopApplicationTransitionError> {
        self.transition(PartnerShopApplicationState::Withdrawn)
    }

    pub fn has_applied_decision(&self, decision: PartnerShopApplicationDecision) -> bool {
        matches!(
            (decision, self.business_state),
            (
                PartnerShopApplicationDecision::Approve,
                PartnerShopApplicationState::Approved
            ) | (
                PartnerShopApplicationDecision::Reject,
                PartnerShopApplicationState::Rejected
            )
        )
    }

    fn transition(
        &mut self,
        target: PartnerShopApplicationState,
    ) -> Result<(), PartnerShopApplicationTransitionError> {
        if !is_allowed_transition(self.business_state, target) {
            return Err(PartnerShopApplicationTransitionError::InvalidTransition {
                from: self.business_state,
                to: target,
            });
        }

        self.business_state = target;
        Ok(())
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

    pub fn payload(&self) -> PartnerShopApplicationPayload {
        self.payload
    }

    pub fn shop_id(&self) -> ShopId {
        match self.payload {
            PartnerShopApplicationPayload::Existing { shop_id }
            | PartnerShopApplicationPayload::New { shop_id } => shop_id,
        }
    }

    pub fn is_new_shop_application(&self) -> bool {
        matches!(self.payload, PartnerShopApplicationPayload::New { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnerShopApplicationDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PartnerShopApplicationTransitionError {
    #[error("cannot transition partner shop application from {from:?} to {to:?}")]
    InvalidTransition {
        from: PartnerShopApplicationState,
        to: PartnerShopApplicationState,
    },
}

fn is_allowed_transition(
    from: PartnerShopApplicationState,
    to: PartnerShopApplicationState,
) -> bool {
    matches!(
        (from, to),
        (
            PartnerShopApplicationState::Submitted,
            PartnerShopApplicationState::InReview
        ) | (
            PartnerShopApplicationState::Submitted,
            PartnerShopApplicationState::Withdrawn
        ) | (
            PartnerShopApplicationState::InReview,
            PartnerShopApplicationState::Approved
        ) | (
            PartnerShopApplicationState::InReview,
            PartnerShopApplicationState::Rejected
        ) | (
            PartnerShopApplicationState::InReview,
            PartnerShopApplicationState::Withdrawn
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_submitted_application_linked_to_new_shop() {
        let shop_id = ShopId::new();
        let application = application(PartnerShopApplicationPayload::New { shop_id });

        assert_eq!(shop_id, application.shop_id());
        assert_eq!(
            PartnerShopApplicationState::Submitted,
            application.business_state()
        );
        assert!(application.is_new_shop_application());
    }

    #[test]
    fn should_allow_submitted_to_in_review_to_approved() {
        let mut application = application(PartnerShopApplicationPayload::Existing {
            shop_id: ShopId::new(),
        });

        assert_eq!(Ok(()), application.mark_in_review());
        assert_eq!(Ok(()), application.approve());
        assert_eq!(
            PartnerShopApplicationState::Approved,
            application.business_state()
        );
    }

    #[test]
    fn should_allow_withdrawal_before_or_during_review() {
        let mut submitted = application(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        assert_eq!(Ok(()), submitted.withdraw());

        let mut reviewed = application(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        assert_eq!(Ok(()), reviewed.mark_in_review());
        assert_eq!(Ok(()), reviewed.withdraw());

        assert_eq!(
            PartnerShopApplicationState::Withdrawn,
            submitted.business_state()
        );
        assert_eq!(
            PartnerShopApplicationState::Withdrawn,
            reviewed.business_state()
        );
    }

    #[test]
    fn should_reject_decision_without_review() {
        let mut application = application(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });

        let result = application.approve();

        assert!(matches!(
            result,
            Err(PartnerShopApplicationTransitionError::InvalidTransition {
                from: PartnerShopApplicationState::Submitted,
                to: PartnerShopApplicationState::Approved,
            })
        ));
    }

    #[test]
    fn should_reject_transitions_after_terminal_state() {
        let mut application = application(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        assert_eq!(Ok(()), application.mark_in_review());
        assert_eq!(Ok(()), application.reject());

        assert!(matches!(
            application.approve(),
            Err(PartnerShopApplicationTransitionError::InvalidTransition {
                from: PartnerShopApplicationState::Rejected,
                to: PartnerShopApplicationState::Approved,
            })
        ));
        assert!(matches!(
            application.withdraw(),
            Err(PartnerShopApplicationTransitionError::InvalidTransition {
                from: PartnerShopApplicationState::Rejected,
                to: PartnerShopApplicationState::Withdrawn,
            })
        ));
    }

    #[test]
    fn should_identify_only_matching_terminal_decision_as_replayable() {
        let mut application = application(PartnerShopApplicationPayload::Existing {
            shop_id: ShopId::new(),
        });
        assert_eq!(Ok(()), application.mark_in_review());
        assert_eq!(Ok(()), application.approve());

        assert!(application.has_applied_decision(PartnerShopApplicationDecision::Approve));
        assert!(!application.has_applied_decision(PartnerShopApplicationDecision::Reject));
    }

    fn application(payload: PartnerShopApplicationPayload) -> PartnerShopApplication {
        PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
            payload,
        })
    }
}
