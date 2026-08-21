use crate::{
    core::{
        command::{
            CreatePartnerShopApplicationCommand, CreatePartnerShopApplicationPayload,
            PartnerShopApplicationDecision, UpdatePartnerShopApplicationCommand,
        },
        partner_shop_application::{
            CreateShopCommand, PartnerShopApplication, PartnerShopApplicationPayload,
        },
        partner_shop_application_id::PartnerShopApplicationId,
    },
    dynamodb::{
        partner_shop_application_payload_type_record::PartnerShopApplicationPayloadTypeRecord,
        partner_shop_application_record::{
            PartnerShopApplicationRecord, structured_address_from_flat,
        },
        partner_shop_application_record_update::PartnerShopApplicationRecordUpdate,
        repository::PartnerShopApplicationDynamoDbRepository,
    },
};
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::{actor::RequestContext, user_id::UserId};
use shop::core::partner_status::ShopPartnerStatus;
use shop::service::get_service::{GetShopError, GetShopService};
use time::OffsetDateTime;
use tracing::info;

#[derive(thiserror::Error, Debug)]
pub enum PartnerShopApplicationError {
    #[error("There exists no PartnerShopApplication for user '{0}' with id '{1}'.")]
    NotFound(UserId, PartnerShopApplicationId),

    #[error("There exists no PartnerShopApplication with id '{0}'.")]
    NotFoundById(PartnerShopApplicationId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0:?}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0:?}")]
    SdkPutItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0:?}")]
    SdkUpdateItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for DeleteItem: {0:?}")]
    SdkDeleteItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError, HttpResponse>,
    ),

    #[error("Missing persistence field: {0}")]
    MissingPersistenceField(#[from] common::error::missing_field::MissingPersistenceField),

    #[error("SFN adapter error: {0}")]
    SfnAdapterError(#[from] crate::service::sfn_adapter::SfnAdapterError),

    #[error("Application not in review state - cannot resume step function for id '{0}'.")]
    NotInReviewState(PartnerShopApplicationId),

    #[error("Missing task token for application '{0}'.")]
    MissingTaskToken(PartnerShopApplicationId),

    #[error("Failed hydrating existing shop payload: {0}")]
    GetShopError(#[from] GetShopError),
}

#[cfg(feature = "data")]
pub mod api {
    use super::PartnerShopApplicationError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        CONFLICT, INTERNAL_SERVER_ERROR, PARTNER_SHOP_APPLICATION_NOT_FOUND,
    };

    impl From<PartnerShopApplicationError> for ApiError {
        fn from(err: PartnerShopApplicationError) -> Self {
            match err {
                PartnerShopApplicationError::NotFound(_, _) => {
                    ApiError::not_found(PARTNER_SHOP_APPLICATION_NOT_FOUND, Box::new(err))
                }
                PartnerShopApplicationError::NotFoundById(_) => {
                    ApiError::not_found(PARTNER_SHOP_APPLICATION_NOT_FOUND, Box::new(err))
                }
                PartnerShopApplicationError::SdkGetItemError(sdk_error) => sdk_error.into(),
                PartnerShopApplicationError::SdkQueryError(sdk_error) => sdk_error.into(),
                PartnerShopApplicationError::SdkPutItemError(sdk_error) => sdk_error.into(),
                PartnerShopApplicationError::SdkUpdateItemError(sdk_error) => sdk_error.into(),
                PartnerShopApplicationError::SdkDeleteItemError(sdk_error) => sdk_error.into(),
                PartnerShopApplicationError::MissingPersistenceField(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                PartnerShopApplicationError::SfnAdapterError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                PartnerShopApplicationError::NotInReviewState(_) => {
                    ApiError::conflict(CONFLICT, Box::new(err))
                }
                PartnerShopApplicationError::MissingTaskToken(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                PartnerShopApplicationError::GetShopError(get_shop_error) => {
                    let msg = get_shop_error.to_string();
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(get_shop_error))
                        .with_detail(msg)
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PartnerShopApplicationService {
    async fn create_partner_shop_application(
        &self,
        ctx: &RequestContext,
        cmd: CreatePartnerShopApplicationCommand,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError>;

    async fn find_partner_shop_application(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError>;

    async fn update_partner_shop_application(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
        update: UpdatePartnerShopApplicationCommand,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError>;

    async fn delete_partner_shop_application(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<(), PartnerShopApplicationError>;

    async fn find_all_partner_shop_applications(
        &self,
    ) -> Result<Vec<PartnerShopApplication>, PartnerShopApplicationError>;

    async fn find_partner_shop_application_by_id(
        &self,
        id: &PartnerShopApplicationId,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError>;

    async fn update_partner_shop_application_by_id(
        &self,
        ctx: &RequestContext,
        id: &PartnerShopApplicationId,
        update: UpdatePartnerShopApplicationCommand,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError>;

    async fn submit_decision(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
        decision: PartnerShopApplicationDecision,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError>;

    async fn submit_decision_by_id(
        &self,
        ctx: &RequestContext,
        id: &PartnerShopApplicationId,
        decision: PartnerShopApplicationDecision,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError>;

    async fn find_all_partner_shop_applications_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<PartnerShopApplication>, PartnerShopApplicationError>;
}

pub struct PartnerShopApplicationServiceImpl<'a> {
    repository: &'a (dyn PartnerShopApplicationDynamoDbRepository + Sync),
    shop_service: &'a (dyn GetShopService + Sync),
    sfn_adapter: &'a (dyn crate::service::sfn_adapter::SfnAdapter + Send + Sync),
    state_machine_arn: &'a str,
}

impl<'a> PartnerShopApplicationServiceImpl<'a> {
    pub fn new(
        repository: &'a (dyn PartnerShopApplicationDynamoDbRepository + Sync),
        shop_service: &'a (dyn GetShopService + Sync),
        sfn_adapter: &'a (dyn crate::service::sfn_adapter::SfnAdapter + Send + Sync),
        state_machine_arn: &'a str,
    ) -> Self {
        Self {
            repository,
            shop_service,
            sfn_adapter,
            state_machine_arn,
        }
    }
}

#[async_trait::async_trait]
impl<'a> PartnerShopApplicationService for PartnerShopApplicationServiceImpl<'a> {
    async fn create_partner_shop_application(
        &self,
        ctx: &RequestContext,
        cmd: CreatePartnerShopApplicationCommand,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let now = OffsetDateTime::now_utc();
        let application = PartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            business_state:
                crate::core::partner_shop_application_state::PartnerShopApplicationState::Submitted,
            execution_state: common::execution_state::ExecutionState::Processing,
            applicant_user_id: cmd.applicant_user_id,
            payload: match cmd.payload {
                CreatePartnerShopApplicationPayload::Existing(shop_id) => {
                    PartnerShopApplicationPayload::Existing(
                        self.shop_service.find_shop(&shop_id).await?,
                    )
                }
                CreatePartnerShopApplicationPayload::New(cmd) => {
                    PartnerShopApplicationPayload::New(cmd)
                }
            },
            created_by: ctx.actor,
            updated_by: ctx.actor,
            created: now,
            updated: now,
        };
        let record = PartnerShopApplicationRecord::from(application.clone());
        self.repository
            .put_partner_shop_application_record(record)
            .await?;

        let sfn_input = serde_json::json!({
            "partner_application_id": application.id.to_string(),
            "applicant_user_id": application.applicant_user_id.to_string(),
        });
        self.sfn_adapter
            .start_execution(self.state_machine_arn, &sfn_input.to_string())
            .await?;

        info!(
            actor = %ctx.actor,
            partnerShopApplicationId = %application.id,
            userId = %application.applicant_user_id,
            "PartnerShopApplication created and step function started."
        );

        Ok(application)
    }

    async fn find_partner_shop_application(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let record = self
            .repository
            .get_partner_shop_application_record(user_id, id)
            .await?
            .ok_or(PartnerShopApplicationError::NotFound(*user_id, *id))?;

        self.hydrate_record(record).await
    }

    async fn update_partner_shop_application(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
        update: UpdatePartnerShopApplicationCommand,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let existing_record = self
            .repository
            .get_partner_shop_application_record(user_id, id)
            .await?
            .ok_or(PartnerShopApplicationError::NotFound(*user_id, *id))?;

        if update.is_empty() {
            return self.hydrate_record(existing_record).await;
        }

        let record_update = PartnerShopApplicationRecordUpdate {
            business_state: None,
            execution_state: None,
            shop_name: update.shop_name,
            shop_type: update.shop_type.map(Into::into),
            shop_domains: update.shop_domains,
            shop_url: update.shop_url,
            shop_image: update.shop_image,
            shop_structured_address_addressline: update
                .shop_structured_address
                .as_ref()
                .and_then(|a| a.addressline.clone()),
            shop_structured_address_addressline_extra: update
                .shop_structured_address
                .as_ref()
                .and_then(|a| a.addressline_extra.clone()),
            shop_structured_address_locality: update
                .shop_structured_address
                .as_ref()
                .and_then(|address| address.locality.clone()),
            shop_structured_address_region: update
                .shop_structured_address
                .as_ref()
                .and_then(|address| address.region.clone()),
            shop_structured_address_postal_code: update
                .shop_structured_address
                .as_ref()
                .and_then(|address| address.postal_code.clone()),
            shop_structured_address_country: update
                .shop_structured_address
                .as_ref()
                .and_then(|a| a.country),
            shop_phone: update.shop_phone,
            shop_email: update.shop_email,
            task_token: None,
            updated_by: ctx.actor.into(),
            updated: OffsetDateTime::now_utc(),
        };

        let updated_record = self
            .repository
            .update_partner_shop_application_record(user_id, id, record_update)
            .await?
            .ok_or_else(|| {
                PartnerShopApplicationError::SdkUpdateItemError(SdkError::construction_failure(
                    "Failed retrieving new PartnerShopApplication on update",
                ))
            })?;

        info!(
            actor = %ctx.actor,
            partnerShopApplicationId = %id,
            userId = %user_id,
            "PartnerShopApplication updated."
        );

        self.hydrate_record(updated_record).await
    }

    async fn delete_partner_shop_application(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<(), PartnerShopApplicationError> {
        self.repository
            .get_partner_shop_application_record(user_id, id)
            .await?
            .ok_or(PartnerShopApplicationError::NotFound(*user_id, *id))?;

        self.repository
            .delete_partner_shop_application_record(user_id, id)
            .await?;

        info!(
            actor = %ctx.actor,
            partnerShopApplicationId = %id,
            userId = %user_id,
            "PartnerShopApplication deleted."
        );

        Ok(())
    }

    async fn find_all_partner_shop_applications(
        &self,
    ) -> Result<Vec<PartnerShopApplication>, PartnerShopApplicationError> {
        let records = self
            .repository
            .query_all_partner_shop_application_records()
            .await?;

        let mut applications = Vec::with_capacity(records.len());
        for record in records {
            applications.push(self.hydrate_record(record).await?);
        }

        Ok(applications)
    }

    async fn find_partner_shop_application_by_id(
        &self,
        id: &PartnerShopApplicationId,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let record = self
            .repository
            .query_partner_shop_application_record_by_id(id)
            .await?
            .ok_or(PartnerShopApplicationError::NotFoundById(*id))?;

        self.hydrate_record(record).await
    }

    async fn update_partner_shop_application_by_id(
        &self,
        ctx: &RequestContext,
        id: &PartnerShopApplicationId,
        update: UpdatePartnerShopApplicationCommand,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let existing_record = self
            .repository
            .query_partner_shop_application_record_by_id(id)
            .await?
            .ok_or(PartnerShopApplicationError::NotFoundById(*id))?;

        if update.is_empty() {
            return self.hydrate_record(existing_record).await;
        }

        let user_id = existing_record.applicant_user_id;

        let record_update = PartnerShopApplicationRecordUpdate {
            business_state: None,
            execution_state: None,
            shop_name: update.shop_name,
            shop_type: update.shop_type.map(Into::into),
            shop_domains: update.shop_domains,
            shop_url: update.shop_url,
            shop_image: update.shop_image,
            shop_structured_address_addressline: update
                .shop_structured_address
                .as_ref()
                .and_then(|a| a.addressline.clone()),
            shop_structured_address_addressline_extra: update
                .shop_structured_address
                .as_ref()
                .and_then(|a| a.addressline_extra.clone()),
            shop_structured_address_locality: update
                .shop_structured_address
                .as_ref()
                .and_then(|address| address.locality.clone()),
            shop_structured_address_region: update
                .shop_structured_address
                .as_ref()
                .and_then(|address| address.region.clone()),
            shop_structured_address_postal_code: update
                .shop_structured_address
                .as_ref()
                .and_then(|address| address.postal_code.clone()),
            shop_structured_address_country: update
                .shop_structured_address
                .as_ref()
                .and_then(|a| a.country),
            shop_phone: update.shop_phone,
            shop_email: update.shop_email,
            task_token: None,
            updated_by: ctx.actor.into(),
            updated: OffsetDateTime::now_utc(),
        };

        let updated_record = self
            .repository
            .update_partner_shop_application_record(&user_id, id, record_update)
            .await?
            .ok_or_else(|| {
                PartnerShopApplicationError::SdkUpdateItemError(SdkError::construction_failure(
                    "Failed retrieving new PartnerShopApplication on update",
                ))
            })?;

        info!(
            actor = %ctx.actor,
            partnerShopApplicationId = %id,
            userId = %user_id,
            "PartnerShopApplication updated by admin."
        );

        self.hydrate_record(updated_record).await
    }

    async fn submit_decision(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
        decision: PartnerShopApplicationDecision,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let existing_record = self
            .repository
            .get_partner_shop_application_record(user_id, id)
            .await?
            .ok_or(PartnerShopApplicationError::NotFound(*user_id, *id))?;

        self.resume_step_function(ctx, id, &existing_record, decision)
            .await
    }

    async fn submit_decision_by_id(
        &self,
        ctx: &RequestContext,
        id: &PartnerShopApplicationId,
        decision: PartnerShopApplicationDecision,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let existing_record = self
            .repository
            .query_partner_shop_application_record_by_id(id)
            .await?
            .ok_or(PartnerShopApplicationError::NotFoundById(*id))?;

        self.resume_step_function(ctx, id, &existing_record, decision)
            .await
    }

    async fn find_all_partner_shop_applications_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<PartnerShopApplication>, PartnerShopApplicationError> {
        let records = self
            .repository
            .query_all_partner_shop_application_records_by_user(user_id)
            .await?;

        let mut applications = Vec::with_capacity(records.len());
        for record in records {
            applications.push(self.hydrate_record(record).await?);
        }

        Ok(applications)
    }
}

impl<'a> PartnerShopApplicationServiceImpl<'a> {
    // Preserve the legacy AWS/Shop error API by value during this crate's migration.
    #[allow(clippy::result_large_err)]
    async fn hydrate_record(
        &self,
        record: PartnerShopApplicationRecord,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        let payload = match record.payload_type {
            PartnerShopApplicationPayloadTypeRecord::Existing => {
                let shop_id = record.existing_shop_id.ok_or_else(|| {
                    common::error::missing_field::MissingPersistenceField::new("existing_shop_id")
                })?;
                PartnerShopApplicationPayload::Existing(
                    self.shop_service.find_shop(&shop_id).await?,
                )
            }
            PartnerShopApplicationPayloadTypeRecord::New => {
                let name = record.shop_name.ok_or_else(|| {
                    common::error::missing_field::MissingPersistenceField::new("shop_name")
                })?;
                let shop_type_record = record.shop_type.ok_or_else(|| {
                    common::error::missing_field::MissingPersistenceField::new("shop_type")
                })?;

                PartnerShopApplicationPayload::New(CreateShopCommand {
                    name,
                    shop_type: shop_type_record.into(),
                    shop_partner_status: ShopPartnerStatus::Partnered,
                    domains: record.shop_domains.unwrap_or_default(),
                    shopify_domain: None,
                    shopify_currency: None,
                    shopify_language: None,
                    woocommerce_webhook_secret: None,
                    woocommerce_currency: None,
                    woocommerce_language: None,
                    url: record.shop_url,
                    image: record.shop_image,
                    structured_address: structured_address_from_flat(
                        record.shop_structured_address_addressline,
                        record.shop_structured_address_addressline_extra,
                        record.shop_structured_address_locality,
                        record.shop_structured_address_region,
                        record.shop_structured_address_postal_code,
                        record.shop_structured_address_country,
                    ),
                    phone: record.shop_phone,
                    email: record.shop_email,
                    affiliate_configuration: None,
                })
            }
        };

        Ok(PartnerShopApplication {
            id: record.id,
            business_state: record.business_state.into(),
            execution_state: record.execution_state.into(),
            applicant_user_id: record.applicant_user_id,
            payload,
            created_by: record.created_by.into(),
            updated_by: record.updated_by.into(),
            created: record.created,
            updated: record.updated,
        })
    }

    // Preserve the legacy AWS/Shop error API by value during this crate's migration.
    #[allow(clippy::result_large_err)]
    async fn resume_step_function(
        &self,
        ctx: &RequestContext,
        id: &PartnerShopApplicationId,
        existing_record: &PartnerShopApplicationRecord,
        decision: PartnerShopApplicationDecision,
    ) -> Result<PartnerShopApplication, PartnerShopApplicationError> {
        use crate::core::partner_shop_application_state::PartnerShopApplicationState;

        let current_state: PartnerShopApplicationState = existing_record.business_state.into();
        if current_state != PartnerShopApplicationState::InReview {
            return Err(PartnerShopApplicationError::NotInReviewState(*id));
        }

        let task_token = existing_record
            .task_token
            .clone()
            .ok_or(PartnerShopApplicationError::MissingTaskToken(*id))?;

        let decision_str = match decision {
            PartnerShopApplicationDecision::Approve => "APPROVED",
            PartnerShopApplicationDecision::Reject => "REJECTED",
        };

        let user_id = existing_record.applicant_user_id;
        let output = serde_json::json!({
            "decision": decision_str,
            "partner_application_id": id.to_string(),
            "applicant_user_id": user_id.to_string(),
        });
        self.sfn_adapter
            .send_task_success(&task_token, &output.to_string())
            .await?;

        // Set execution_state to Processing while step function runs the decision
        let record_update = PartnerShopApplicationRecordUpdate {
            business_state: None,
            execution_state: Some(
                common::execution_state::record::ExecutionStateRecord::Processing,
            ),
            shop_name: None,
            shop_type: None,
            shop_domains: None,
            shop_url: None,
            shop_image: None,
            shop_structured_address_addressline: None,
            shop_structured_address_addressline_extra: None,
            shop_structured_address_locality: None,
            shop_structured_address_region: None,
            shop_structured_address_postal_code: None,
            shop_structured_address_country: None,
            shop_phone: None,
            shop_email: None,
            task_token: None,
            updated_by: ctx.actor.into(),
            updated: OffsetDateTime::now_utc(),
        };

        let updated_record = self
            .repository
            .update_partner_shop_application_record(&user_id, id, record_update)
            .await?
            .ok_or_else(|| {
                PartnerShopApplicationError::SdkUpdateItemError(SdkError::construction_failure(
                    "Failed retrieving PartnerShopApplication after setting Processing state",
                ))
            })?;

        info!(
            actor = %ctx.actor,
            partnerShopApplicationId = %id,
            decision = decision_str,
            "Step function resumed with decision, execution_state set to Processing."
        );

        self.hydrate_record(updated_record).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{
            command::CreatePartnerShopApplicationPayload,
            partner_shop_application::PartnerShopApplicationPayload,
            partner_shop_application_state::PartnerShopApplicationState,
        },
        dynamodb::repository::MockPartnerShopApplicationDynamoDbRepository,
    };
    use aws_sdk_dynamodb::{
        config::http::HttpResponse as DynamoHttpResponse,
        error::{ConnectorError, SdkError as DynamoSdkError},
        operation::delete_item::DeleteItemOutput,
        operation::put_item::PutItemOutput,
    };
    use common::{execution_state::ExecutionState, shop_id::ShopId, user_id::UserId};
    use fake::{Fake, Faker};
    use shop::service::get_service::MockGetShopService;

    fn make_service<'a>(
        repository: &'a MockPartnerShopApplicationDynamoDbRepository,
        sfn_adapter: &'a crate::service::sfn_adapter::MockSfnAdapter,
    ) -> PartnerShopApplicationServiceImpl<'a> {
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_shop()
            .returning(|_| Box::pin(async { Ok(Faker.fake()) }));
        let shop_service = Box::leak(Box::new(shop_service));
        PartnerShopApplicationServiceImpl::new(
            repository,
            shop_service,
            sfn_adapter,
            "arn:aws:states:us-east-1:123456789:stateMachine:test",
        )
    }

    fn system_ctx() -> RequestContext {
        RequestContext {
            actor: common::actor::domain::Actor::System,
        }
    }

    mod create {
        use super::*;

        #[tokio::test]
        async fn should_create_partner_shop_application() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_put_partner_shop_application_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let mut sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            sfn_adapter
                .expect_start_execution()
                .return_once(|_, _| Box::pin(async { Ok("execution-arn".to_string()) }));
            let service = make_service(&repository, &sfn_adapter);
            let cmd = CreatePartnerShopApplicationCommand {
                applicant_user_id: UserId::new(),
                payload: CreatePartnerShopApplicationPayload::Existing(ShopId::new()),
            };

            let actual = service
                .create_partner_shop_application(&system_ctx(), cmd.clone())
                .await
                .unwrap();

            assert_eq!(actual.applicant_user_id, cmd.applicant_user_id);
            assert!(matches!(
                actual.payload,
                PartnerShopApplicationPayload::Existing(_)
            ));
            assert_eq!(
                actual.business_state,
                PartnerShopApplicationState::Submitted
            );
            assert_eq!(actual.execution_state, ExecutionState::Processing);
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_put(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                DynamoHttpResponse,
            >,
        ) {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_put_partner_shop_application_record()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let cmd: CreatePartnerShopApplicationCommand = Faker.fake();
            let actual = service
                .create_partner_shop_application(&system_ctx(), cmd)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                PartnerShopApplicationError::SdkPutItemError(_) => {}
                err => panic!("Expected 'SdkPutItemError', got '{err}'"),
            }
        }
    }

    mod find {
        use super::*;

        #[tokio::test]
        async fn should_find_partner_shop_application() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let expected: PartnerShopApplication = Faker.fake();
            let record = PartnerShopApplicationRecord::from(expected.clone());

            repository
                .expect_get_partner_shop_application_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .find_partner_shop_application(&expected.applicant_user_id, &expected.id)
                .await
                .unwrap();

            assert_eq!(expected.id, actual.id);
            assert_eq!(expected.business_state, actual.business_state);
            assert_eq!(expected.execution_state, actual.execution_state);
            assert_eq!(expected.applicant_user_id, actual.applicant_user_id);
        }

        #[tokio::test]
        async fn should_hydrate_existing_shop_payload_when_finding_partner_shop_application() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let expected_shop: shop::core::shop::Shop = Faker.fake();
            let mut record: PartnerShopApplicationRecord = Faker.fake();
            record.payload_type = crate::dynamodb::partner_shop_application_payload_type_record::PartnerShopApplicationPayloadTypeRecord::Existing;
            record.existing_shop_id = Some(expected_shop.shop_id);
            record.shop_name = None;
            record.shop_type = None;
            record.shop_domains = None;

            repository
                .expect_get_partner_shop_application_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let mut shop_service = MockGetShopService::default();
            let expected_shop_for_match = expected_shop.clone();
            let expected_shop_for_assertion = expected_shop.clone();
            shop_service
                .expect_find_shop()
                .withf(move |shop_id| *shop_id == expected_shop_for_match.shop_id)
                .return_once(move |_| Box::pin(async move { Ok(expected_shop) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = PartnerShopApplicationServiceImpl::new(
                &repository,
                &shop_service,
                &sfn_adapter,
                "arn:aws:states:us-east-1:123456789:stateMachine:test",
            );

            let actual = service
                .find_partner_shop_application(&UserId::new(), &PartnerShopApplicationId::new())
                .await
                .unwrap();

            assert!(matches!(
                actual.payload,
                PartnerShopApplicationPayload::Existing(shop) if shop == expected_shop_for_assertion
            ));
        }

        #[tokio::test]
        async fn should_err_not_found_when_no_application_exists() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_get_partner_shop_application_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let user_id = UserId::new();
            let id = PartnerShopApplicationId::new();
            let actual = service
                .find_partner_shop_application(&user_id, &id)
                .await
                .unwrap_err();

            match actual {
                PartnerShopApplicationError::NotFound(err_user_id, err_id) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(id, err_id);
                }
                err => panic!("Expected 'NotFound', got '{err}'"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                DynamoHttpResponse,
            >,
        ) {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_get_partner_shop_application_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .find_partner_shop_application(&UserId::new(), &PartnerShopApplicationId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                PartnerShopApplicationError::SdkGetItemError(_) => {}
                err => panic!("Expected 'SdkGetItemError', got '{err}'"),
            }
        }
    }

    mod update {
        use super::*;

        #[tokio::test]
        async fn should_return_existing_when_update_is_empty() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let expected: PartnerShopApplication = Faker.fake();
            let record = PartnerShopApplicationRecord::from(expected.clone());

            repository
                .expect_get_partner_shop_application_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .update_partner_shop_application(
                    &system_ctx(),
                    &expected.applicant_user_id,
                    &expected.id,
                    UpdatePartnerShopApplicationCommand::default(),
                )
                .await
                .unwrap();

            assert_eq!(expected.id, actual.id);
        }

        #[tokio::test]
        async fn should_update_partner_shop_application_shop_name() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let expected: PartnerShopApplication = Faker.fake();
            let record = PartnerShopApplicationRecord::from(expected.clone());

            let updated_record = record.clone();

            repository
                .expect_get_partner_shop_application_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            repository
                .expect_update_partner_shop_application_record()
                .return_once(move |_, _, _| Box::pin(async move { Ok(Some(updated_record)) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .update_partner_shop_application(
                    &system_ctx(),
                    &expected.applicant_user_id,
                    &expected.id,
                    UpdatePartnerShopApplicationCommand {
                        shop_name: Some(common::shop_name::ShopName::from(
                            "Updated Name".to_string(),
                        )),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            assert_eq!(expected.id, actual.id);
        }

        #[tokio::test]
        async fn should_err_not_found_when_updating_non_existing_application() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_get_partner_shop_application_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let user_id = UserId::new();
            let id = PartnerShopApplicationId::new();
            let actual = service
                .update_partner_shop_application(
                    &system_ctx(),
                    &user_id,
                    &id,
                    UpdatePartnerShopApplicationCommand {
                        shop_name: Some(common::shop_name::ShopName::from("Test".to_string())),
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();

            match actual {
                PartnerShopApplicationError::NotFound(err_user_id, err_id) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(id, err_id);
                }
                err => panic!("Expected 'NotFound', got '{err}'"),
            }
        }
    }

    mod delete {
        use super::*;

        #[tokio::test]
        async fn should_delete_partner_shop_application() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let expected: PartnerShopApplication = Faker.fake();
            let record = PartnerShopApplicationRecord::from(expected.clone());

            repository
                .expect_get_partner_shop_application_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            repository
                .expect_delete_partner_shop_application_record()
                .return_once(|_, _| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .delete_partner_shop_application(
                    &system_ctx(),
                    &expected.applicant_user_id,
                    &expected.id,
                )
                .await;

            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_err_not_found_when_deleting_non_existing_application() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_get_partner_shop_application_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let user_id = UserId::new();
            let id = PartnerShopApplicationId::new();
            let actual = service
                .delete_partner_shop_application(&system_ctx(), &user_id, &id)
                .await
                .unwrap_err();

            match actual {
                PartnerShopApplicationError::NotFound(err_user_id, err_id) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(id, err_id);
                }
                err => panic!("Expected 'NotFound', got '{err}'"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::delete_item::DeleteItemError::unhandled("Something went wrong"),
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_delete(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::delete_item::DeleteItemError,
                DynamoHttpResponse,
            >,
        ) {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let existing: PartnerShopApplication = Faker.fake();
            let record = PartnerShopApplicationRecord::from(existing.clone());

            repository
                .expect_get_partner_shop_application_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));
            repository
                .expect_delete_partner_shop_application_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .delete_partner_shop_application(
                    &system_ctx(),
                    &existing.applicant_user_id,
                    &existing.id,
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                PartnerShopApplicationError::SdkDeleteItemError(_) => {}
                err => panic!("Expected 'SdkDeleteItemError', got '{err}'"),
            }
        }
    }

    mod find_all {
        use super::*;

        #[tokio::test]
        async fn should_find_all_partner_shop_applications() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let applications: Vec<PartnerShopApplication> = (0..3).map(|_| Faker.fake()).collect();
            let records: Vec<PartnerShopApplicationRecord> = applications
                .iter()
                .cloned()
                .map(PartnerShopApplicationRecord::from)
                .collect();

            repository
                .expect_query_all_partner_shop_application_records()
                .return_once(move || Box::pin(async move { Ok(records) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service.find_all_partner_shop_applications().await.unwrap();

            assert_eq!(3, actual.len());
        }

        #[tokio::test]
        async fn should_return_empty_when_no_applications_exist() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_query_all_partner_shop_application_records()
                .return_once(|| Box::pin(async { Ok(vec![]) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service.find_all_partner_shop_applications().await.unwrap();

            assert!(actual.is_empty());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::query::QueryError::unhandled("Something went wrong"),
            DynamoHttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_query(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                DynamoHttpResponse,
            >,
        ) {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_query_all_partner_shop_application_records()
                .return_once(|| Box::pin(async { Err(expected) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service.find_all_partner_shop_applications().await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                PartnerShopApplicationError::SdkQueryError(_) => {}
                err => panic!("Expected 'SdkQueryError', got '{err}'"),
            }
        }
    }

    mod find_all_by_user {
        use super::*;

        #[tokio::test]
        async fn should_find_all_partner_shop_applications_by_user() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            let user_id = UserId::new();
            let applications: Vec<PartnerShopApplication> = (0..3).map(|_| Faker.fake()).collect();
            let records: Vec<PartnerShopApplicationRecord> = applications
                .iter()
                .cloned()
                .map(PartnerShopApplicationRecord::from)
                .collect();

            repository
                .expect_query_all_partner_shop_application_records_by_user()
                .return_once(move |_| Box::pin(async move { Ok(records) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .find_all_partner_shop_applications_by_user(&user_id)
                .await
                .unwrap();

            assert_eq!(3, actual.len());
        }

        #[tokio::test]
        async fn should_return_empty_when_no_applications_exist_for_user() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_query_all_partner_shop_application_records_by_user()
                .return_once(|_| Box::pin(async { Ok(vec![]) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .find_all_partner_shop_applications_by_user(&UserId::new())
                .await
                .unwrap();

            assert!(actual.is_empty());
        }
    }

    mod create_starts_step_function {
        use super::*;

        #[tokio::test]
        async fn should_start_step_function_on_create() {
            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_put_partner_shop_application_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let state_machine_arn =
                "arn:aws:states:us-east-1:123456789:stateMachine:test".to_string();
            let expected_arn = state_machine_arn.clone();

            let mut sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            sfn_adapter
                .expect_start_execution()
                .withf(move |arn, _input| arn == expected_arn)
                .return_once(|_, _| Box::pin(async { Ok("execution-arn".to_string()) }));

            let service = make_service(&repository, &sfn_adapter);
            let cmd = CreatePartnerShopApplicationCommand {
                applicant_user_id: UserId::new(),
                payload: CreatePartnerShopApplicationPayload::Existing(ShopId::new()),
            };

            let actual = service
                .create_partner_shop_application(&system_ctx(), cmd)
                .await;
            assert!(actual.is_ok());
            let app = actual.unwrap();
            assert_eq!(app.business_state, PartnerShopApplicationState::Submitted);
            assert_eq!(app.execution_state, ExecutionState::Processing);
        }
    }

    mod submit_decision_tests {
        use super::*;
        use crate::dynamodb::partner_shop_application_state_record::PartnerShopApplicationStateRecord;

        #[tokio::test]
        async fn should_resume_step_function_when_approving_in_review_application() {
            let id = PartnerShopApplicationId::new();
            let task_token = "my-task-token-abc".to_string();

            let mut record: PartnerShopApplicationRecord = Faker.fake();
            record.id = id;
            record.business_state = PartnerShopApplicationStateRecord::InReview;
            record.execution_state = common::execution_state::record::ExecutionStateRecord::Waiting;
            record.task_token = Some(task_token.clone());

            let updated_record = {
                let mut r = record.clone();
                r.execution_state =
                    common::execution_state::record::ExecutionStateRecord::Processing;
                r
            };

            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_query_partner_shop_application_record_by_id()
                .withf(move |query_id| *query_id == id)
                .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

            repository
                .expect_update_partner_shop_application_record()
                .return_once(move |_, _, _| Box::pin(async move { Ok(Some(updated_record)) }));

            let expected_token = task_token.clone();
            let mut sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            sfn_adapter
                .expect_send_task_success()
                .withf(move |token, output| {
                    token == expected_token
                        && output.contains("APPROVED")
                        && output.contains("partner_application_id")
                })
                .return_once(|_, _| Box::pin(async { Ok(()) }));

            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .submit_decision_by_id(&system_ctx(), &id, PartnerShopApplicationDecision::Approve)
                .await;

            assert!(actual.is_ok());
            let app = actual.unwrap();
            assert_eq!(app.execution_state, ExecutionState::Processing);
        }

        #[tokio::test]
        async fn should_resume_step_function_when_rejecting_in_review_application() {
            let id = PartnerShopApplicationId::new();
            let task_token = "my-task-token-xyz".to_string();

            let mut record: PartnerShopApplicationRecord = Faker.fake();
            record.id = id;
            record.business_state = PartnerShopApplicationStateRecord::InReview;
            record.execution_state = common::execution_state::record::ExecutionStateRecord::Waiting;
            record.task_token = Some(task_token.clone());

            let updated_record = {
                let mut r = record.clone();
                r.execution_state =
                    common::execution_state::record::ExecutionStateRecord::Processing;
                r
            };

            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_query_partner_shop_application_record_by_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

            repository
                .expect_update_partner_shop_application_record()
                .return_once(move |_, _, _| Box::pin(async move { Ok(Some(updated_record)) }));

            let expected_token = task_token.clone();
            let mut sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            sfn_adapter
                .expect_send_task_success()
                .withf(move |token, output| {
                    token == expected_token
                        && output.contains("REJECTED")
                        && output.contains("partner_application_id")
                })
                .return_once(|_, _| Box::pin(async { Ok(()) }));

            let service = make_service(&repository, &sfn_adapter);
            let actual = service
                .submit_decision_by_id(&system_ctx(), &id, PartnerShopApplicationDecision::Reject)
                .await;

            assert!(actual.is_ok());
            let app = actual.unwrap();
            assert_eq!(app.execution_state, ExecutionState::Processing);
        }

        #[tokio::test]
        async fn should_fail_when_approving_non_in_review_application() {
            let id = PartnerShopApplicationId::new();

            let mut record: PartnerShopApplicationRecord = Faker.fake();
            record.id = id;
            record.business_state = PartnerShopApplicationStateRecord::Submitted;

            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_query_partner_shop_application_record_by_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);

            let actual = service
                .submit_decision_by_id(&system_ctx(), &id, PartnerShopApplicationDecision::Approve)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                PartnerShopApplicationError::NotInReviewState(err_id) => {
                    assert_eq!(id, err_id);
                }
                err => panic!("Expected 'NotInReviewState', got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_fail_when_task_token_missing_for_approve() {
            let id = PartnerShopApplicationId::new();

            let mut record: PartnerShopApplicationRecord = Faker.fake();
            record.id = id;
            record.business_state = PartnerShopApplicationStateRecord::InReview;
            record.task_token = None;

            let mut repository = MockPartnerShopApplicationDynamoDbRepository::default();
            repository
                .expect_query_partner_shop_application_record_by_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

            let sfn_adapter = crate::service::sfn_adapter::MockSfnAdapter::default();
            let service = make_service(&repository, &sfn_adapter);

            let actual = service
                .submit_decision_by_id(&system_ctx(), &id, PartnerShopApplicationDecision::Approve)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                PartnerShopApplicationError::MissingTaskToken(err_id) => {
                    assert_eq!(id, err_id);
                }
                err => panic!("Expected 'MissingTaskToken', got '{err}'"),
            }
        }
    }
}
