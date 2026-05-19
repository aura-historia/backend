use common::event_id::EventId;
use common::execution_state::record::ExecutionStateRecord;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::user_id::UserId;
use lambda_runtime::LambdaEvent;
use notification::core::notification::{
    NotificationPartnerApplicationPayload, NotificationPayload,
};
use notification::service::command::CreateNotificationCommand;
use notification::service::notification_service::NotificationService;
use partner_shop_application::core::partner_shop_application_id::PartnerShopApplicationId;
use partner_shop_application::dynamodb::partner_shop_application_payload_type_record::PartnerShopApplicationPayloadTypeRecord;
use partner_shop_application::dynamodb::partner_shop_application_record::PartnerShopApplicationRecord;
use partner_shop_application::dynamodb::partner_shop_application_record_update::PartnerShopApplicationRecordUpdate;
use partner_shop_application::dynamodb::partner_shop_application_state_record::PartnerShopApplicationStateRecord;
use partner_shop_application::dynamodb::repository::PartnerShopApplicationDynamoDbRepository;
use serde::{Deserialize, Serialize};
use shop::core::address::StructuredAddress;
use shop::core::continent::Continent;
use shop::dynamodb::repository::ShopDynamoDbRepository;
use shop::dynamodb::shop_record;
use shop::dynamodb::shop_record_update::ShopRecordUpdate;
use shop::service::command::CreateShopCommand;
use shop::service::command_service::CommandShopService;
use time::OffsetDateTime;
use tracing::info;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StepFunctionStep {
    WaitForReview,
    Approve,
    Reject,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StepFunctionInput {
    pub step: StepFunctionStep,
    #[serde(default)]
    pub task_token: Option<String>,
    pub partner_application_id: PartnerShopApplicationId,
    pub applicant_user_id: UserId,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepFunctionOutput {
    pub partner_application_id: PartnerShopApplicationId,
    pub applicant_user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum StepFunctionError {
    #[error("Missing task_token in WAIT_FOR_REVIEW step")]
    MissingTaskToken,

    #[error("Partner application not found: {0}")]
    ApplicationNotFound(PartnerShopApplicationId),

    #[error("DynamoDB update error: {0}")]
    DynamoDbUpdateError(String),

    #[error("DynamoDB query error: {0}")]
    DynamoDbQueryError(String),

    #[error("Shop creation error: {0}")]
    ShopCreationError(String),

    #[error("Notification error: {0}")]
    NotificationError(String),

    #[error("Missing persistence field: {0}")]
    MissingField(String),
}

#[tracing::instrument(
    skip(partner_app_repository, shop_service, shop_repository, notification_service, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    partner_app_repository: &(impl PartnerShopApplicationDynamoDbRepository + Sync),
    shop_service: &(impl CommandShopService + Sync),
    shop_repository: &(impl ShopDynamoDbRepository + Sync),
    notification_service: &(impl NotificationService + Sync),
    event: LambdaEvent<serde_json::Value>,
) -> Result<serde_json::Value, lambda_runtime::Error> {
    let input: StepFunctionInput = serde_json::from_value(event.payload)?;

    info!(
        step = ?input.step,
        partnerApplicationId = %input.partner_application_id,
        applicantUserId = %input.applicant_user_id,
        "Step function handler invoked."
    );

    match input.step {
        StepFunctionStep::WaitForReview => {
            handle_wait_for_review(partner_app_repository, &input).await?;
        }
        StepFunctionStep::Approve => {
            handle_approve(
                partner_app_repository,
                shop_service,
                shop_repository,
                notification_service,
                &input,
            )
            .await?;
        }
        StepFunctionStep::Reject => {
            handle_reject(partner_app_repository, notification_service, &input).await?;
        }
    }

    let output = StepFunctionOutput {
        partner_application_id: input.partner_application_id,
        applicant_user_id: input.applicant_user_id,
    };

    Ok(serde_json::to_value(output)?)
}

async fn handle_wait_for_review(
    repository: &(impl PartnerShopApplicationDynamoDbRepository + Sync),
    input: &StepFunctionInput,
) -> Result<(), StepFunctionError> {
    let task_token = input
        .task_token
        .as_deref()
        .ok_or(StepFunctionError::MissingTaskToken)?;

    let record_update = PartnerShopApplicationRecordUpdate {
        business_state: Some(PartnerShopApplicationStateRecord::InReview),
        execution_state: Some(ExecutionStateRecord::Waiting),
        task_token: Some(task_token.to_string()),
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
        updated: OffsetDateTime::now_utc(),
    };

    repository
        .update_partner_shop_application_record(
            &input.applicant_user_id,
            &input.partner_application_id,
            record_update,
        )
        .await
        .map_err(|e| StepFunctionError::DynamoDbUpdateError(e.to_string()))?;

    info!(
        partnerApplicationId = %input.partner_application_id,
        "Partner application set to InReview with task token stored."
    );

    Ok(())
}

async fn handle_approve(
    repository: &(impl PartnerShopApplicationDynamoDbRepository + Sync),
    shop_service: &(impl CommandShopService + Sync),
    shop_repository: &(impl ShopDynamoDbRepository + Sync),
    notification_service: &(impl NotificationService + Sync),
    input: &StepFunctionInput,
) -> Result<(), StepFunctionError> {
    let record = repository
        .query_partner_shop_application_record_by_id(&input.partner_application_id)
        .await
        .map_err(|e| StepFunctionError::DynamoDbQueryError(e.to_string()))?
        .ok_or(StepFunctionError::ApplicationNotFound(
            input.partner_application_id,
        ))?;

    let (shop_id, shop_name) =
        create_or_resolve_shop(&record, shop_service, shop_repository).await?;

    link_shop_to_partner(shop_repository, &shop_id, &input.applicant_user_id).await?;

    persist_approved_state(repository, input).await?;

    create_approval_notification(notification_service, input, shop_name).await?;

    info!(
        partnerApplicationId = %input.partner_application_id,
        shopId = %shop_id,
        "Partner application approved."
    );

    Ok(())
}

async fn create_or_resolve_shop(
    record: &PartnerShopApplicationRecord,
    shop_service: &(impl CommandShopService + Sync),
    shop_repository: &(impl ShopDynamoDbRepository + Sync),
) -> Result<(ShopId, ShopName), StepFunctionError> {
    match record.payload_type {
        PartnerShopApplicationPayloadTypeRecord::New => {
            let name = record
                .shop_name
                .clone()
                .ok_or_else(|| StepFunctionError::MissingField("shop_name".into()))?;
            let shop_type = record
                .shop_type
                .ok_or_else(|| StepFunctionError::MissingField("shop_type".into()))?;
            let domains = record.shop_domains.clone().unwrap_or_default();
            let image = record.shop_image.clone();

            let cmd = CreateShopCommand {
                name: name.clone(),
                shop_type: shop_type.into(),
                domains,
                shopify_domain: None,
                shopify_currency: None,
                shopify_language: None,
                woocommerce_webhook_secret: None,
                woocommerce_currency: None,
                woocommerce_language: None,
                url: record.shop_url.clone(),
                image,
                structured_address: structured_address_from_record(record),
                phone: record.shop_phone.clone(),
                email: record.shop_email.clone(),
                affiliate_configuration: None,
            };

            let shop = shop_service
                .create(cmd)
                .await
                .map_err(|e| StepFunctionError::ShopCreationError(e.to_string()))?;

            info!(shopId = %shop.shop_id, "Created new shop for partner application.");

            Ok((shop.shop_id, name))
        }
        PartnerShopApplicationPayloadTypeRecord::Existing => {
            let shop_id = record
                .existing_shop_id
                .ok_or_else(|| StepFunctionError::MissingField("existing_shop_id".into()))?;

            let shop_record = shop_repository
                .get_shop_record(&shop_id)
                .await
                .map_err(|e| StepFunctionError::DynamoDbQueryError(e.to_string()))?
                .ok_or_else(|| {
                    StepFunctionError::MissingField(format!("Shop not found: {shop_id}"))
                })?;

            Ok((shop_id, shop_record.name))
        }
    }
}

fn structured_address_from_record(
    record: &PartnerShopApplicationRecord,
) -> Option<StructuredAddress> {
    let country = record.shop_structured_address_country;
    let continent = country.map(Continent::from);
    let structured_address = StructuredAddress {
        addressline: record.shop_structured_address_addressline.clone(),
        addressline_extra: record.shop_structured_address_addressline_extra.clone(),
        locality: record.shop_structured_address_locality.clone(),
        region: record.shop_structured_address_region.clone(),
        postal_code: record.shop_structured_address_postal_code.clone(),
        country,
        continent,
    };
    (!structured_address.is_empty()).then_some(structured_address)
}

async fn link_shop_to_partner(
    shop_repository: &(impl ShopDynamoDbRepository + Sync),
    shop_id: &ShopId,
    applicant_user_id: &UserId,
) -> Result<(), StepFunctionError> {
    let shop_update = ShopRecordUpdate {
        partner_user_id: Some(*applicant_user_id),
        gsi1_pk: Some(shop_record::mk_gsi1_pk(applicant_user_id)),
        gsi1_sk: Some(shop_record::mk_gsi1_sk(shop_id)),
        gsi3_pk: None,
        gsi3_sk: None,
        shop_type: None,
        domains: None,
        shopify_domain: None,
        shopify_currency: None,
        shopify_language: None,
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        view_url: None,
        image: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        phone: None,
        email: None,
        partner_api_key_short: None,
        partner_api_key_long_hash: None,
        updated: OffsetDateTime::now_utc(),
    };

    shop_repository
        .update_shop_record(shop_id, shop_update)
        .await
        .map_err(|e| StepFunctionError::DynamoDbUpdateError(e.to_string()))?;

    info!(shopId = %shop_id, userId = %applicant_user_id, "Linked partner user to shop.");

    Ok(())
}

async fn persist_approved_state(
    repository: &(impl PartnerShopApplicationDynamoDbRepository + Sync),
    input: &StepFunctionInput,
) -> Result<(), StepFunctionError> {
    let record_update = PartnerShopApplicationRecordUpdate {
        business_state: Some(PartnerShopApplicationStateRecord::Approved),
        execution_state: Some(ExecutionStateRecord::Completed),
        task_token: None,
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
        updated: OffsetDateTime::now_utc(),
    };

    repository
        .update_partner_shop_application_record(
            &input.applicant_user_id,
            &input.partner_application_id,
            record_update,
        )
        .await
        .map_err(|e| StepFunctionError::DynamoDbUpdateError(e.to_string()))?;

    Ok(())
}

async fn create_approval_notification(
    notification_service: &(impl NotificationService + Sync),
    input: &StepFunctionInput,
    shop_name: ShopName,
) -> Result<(), StepFunctionError> {
    let origin_event_id = EventId::new();
    let notification_cmd = CreateNotificationCommand {
        user_id: input.applicant_user_id,
        notification_payload: NotificationPayload::PartnerApplication {
            shop_name,
            partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                partner_application_id: input.partner_application_id,
            },
        },
        external: true,
    };

    notification_service
        .create_notification(&origin_event_id, notification_cmd)
        .await
        .map_err(|e| StepFunctionError::NotificationError(e.to_string()))?;

    Ok(())
}

async fn handle_reject(
    repository: &(impl PartnerShopApplicationDynamoDbRepository + Sync),
    notification_service: &(impl NotificationService + Sync),
    input: &StepFunctionInput,
) -> Result<(), StepFunctionError> {
    let record = repository
        .query_partner_shop_application_record_by_id(&input.partner_application_id)
        .await
        .map_err(|e| StepFunctionError::DynamoDbQueryError(e.to_string()))?
        .ok_or(StepFunctionError::ApplicationNotFound(
            input.partner_application_id,
        ))?;

    let shop_name = resolve_shop_name(&record);

    let record_update = PartnerShopApplicationRecordUpdate {
        business_state: Some(PartnerShopApplicationStateRecord::Rejected),
        execution_state: Some(ExecutionStateRecord::Completed),
        task_token: None,
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
        updated: OffsetDateTime::now_utc(),
    };

    repository
        .update_partner_shop_application_record(
            &input.applicant_user_id,
            &input.partner_application_id,
            record_update,
        )
        .await
        .map_err(|e| StepFunctionError::DynamoDbUpdateError(e.to_string()))?;

    let origin_event_id = EventId::new();
    let notification_cmd = CreateNotificationCommand {
        user_id: input.applicant_user_id,
        notification_payload: NotificationPayload::PartnerApplication {
            shop_name,
            partner_application_payload: NotificationPartnerApplicationPayload::Rejected {
                partner_application_id: input.partner_application_id,
            },
        },
        external: true,
    };

    notification_service
        .create_notification(&origin_event_id, notification_cmd)
        .await
        .map_err(|e| StepFunctionError::NotificationError(e.to_string()))?;

    info!(
        partnerApplicationId = %input.partner_application_id,
        "Partner application rejected."
    );

    Ok(())
}

fn resolve_shop_name(record: &PartnerShopApplicationRecord) -> ShopName {
    record
        .shop_name
        .clone()
        .unwrap_or_else(|| ShopName::from("Unknown Shop"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake::{Fake, Faker};
    use notification::core::notification::Notification;
    use notification::core::notification_id::NotificationId;
    use notification::service::notification_service::MockNotificationService;
    use partner_shop_application::dynamodb::partner_shop_application_payload_type_record::PartnerShopApplicationPayloadTypeRecord;
    use partner_shop_application::dynamodb::repository::MockPartnerShopApplicationDynamoDbRepository;
    use rstest::rstest;
    use shop::core::shop::Shop;
    use shop::dynamodb::repository::MockShopDynamoDbRepository;
    use shop::dynamodb::shop_record::ShopRecord;
    use shop::dynamodb::shop_type_record::ShopTypeRecord;
    use shop::service::command_service::MockCommandShopService;

    fn fake_notification(user_id: UserId) -> Notification {
        Notification {
            user_id,
            origin_event_id: EventId::new(),
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: Faker.fake(),
            seen: false,
            external: true,
            created: time::OffsetDateTime::now_utc(),
            updated: time::OffsetDateTime::now_utc(),
        }
    }

    #[rstest]
    #[case("WAIT_FOR_REVIEW", StepFunctionStep::WaitForReview)]
    #[case("APPROVE", StepFunctionStep::Approve)]
    #[case("REJECT", StepFunctionStep::Reject)]
    fn should_deserialize_step_when_valid_for_step_function_input(
        #[case] input: &str,
        #[case] expected: StepFunctionStep,
    ) {
        let json = format!(
            r#"{{"step":"{input}","partner_application_id":"{}","applicant_user_id":"{}"}}"#,
            PartnerShopApplicationId::new(),
            UserId::new()
        );
        let parsed: StepFunctionInput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.step, expected);
    }

    #[test]
    fn should_fail_deserialization_when_invalid_step_for_step_function_input() {
        let json = format!(
            r#"{{"step":"INVALID","partner_application_id":"{}","applicant_user_id":"{}"}}"#,
            PartnerShopApplicationId::new(),
            UserId::new()
        );
        let result: Result<StepFunctionInput, _> = serde_json::from_str(&json);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn should_fail_when_missing_task_token_for_wait_for_review() {
        let mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();

        let input = StepFunctionInput {
            step: StepFunctionStep::WaitForReview,
            task_token: None,
            partner_application_id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
        };

        let result = handle_wait_for_review(&mock_repo, &input).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StepFunctionError::MissingTaskToken
        ));
    }

    #[tokio::test]
    async fn should_update_record_to_in_review_when_task_token_present_for_wait_for_review() {
        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_update_partner_shop_application_record()
            .withf(|_user_id, _id, update| {
                update.business_state == Some(PartnerShopApplicationStateRecord::InReview)
                    && update.task_token == Some("test-token".to_string())
            })
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let input = StepFunctionInput {
            step: StepFunctionStep::WaitForReview,
            task_token: Some("test-token".to_string()),
            partner_application_id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
        };

        let result = handle_wait_for_review(&mock_repo, &input).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_return_error_when_application_not_found_for_reject() {
        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));
        let mock_notification = MockNotificationService::new();

        let input = StepFunctionInput {
            step: StepFunctionStep::Reject,
            task_token: None,
            partner_application_id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
        };

        let result = handle_reject(&mock_repo, &mock_notification, &input).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StepFunctionError::ApplicationNotFound(_)
        ));
    }

    #[tokio::test]
    async fn should_return_error_when_application_not_found_for_approve() {
        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));

        let mock_shop_service = MockCommandShopService::new();
        let mock_shop_repo = MockShopDynamoDbRepository::new();
        let mock_notification = MockNotificationService::new();

        let input = StepFunctionInput {
            step: StepFunctionStep::Approve,
            task_token: None,
            partner_application_id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
        };

        let result = handle_approve(
            &mock_repo,
            &mock_shop_service,
            &mock_shop_repo,
            &mock_notification,
            &input,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StepFunctionError::ApplicationNotFound(_)
        ));
    }

    #[test]
    fn should_serialize_output_when_valid_for_step_function_output() {
        let output = StepFunctionOutput {
            partner_application_id: PartnerShopApplicationId::new(),
            applicant_user_id: UserId::new(),
        };

        let json = serde_json::to_value(&output).unwrap();
        assert!(json.get("partner_application_id").is_some());
        assert!(json.get("applicant_user_id").is_some());
    }

    #[test]
    fn should_return_unknown_shop_name_when_shop_name_missing_for_resolve() {
        let mut record: PartnerShopApplicationRecord = Faker.fake();
        record.shop_name = None;

        let result = resolve_shop_name(&record);
        assert_eq!(result, ShopName::from("Unknown Shop"));
    }

    #[test]
    fn should_return_shop_name_when_present_for_resolve() {
        let mut record: PartnerShopApplicationRecord = Faker.fake();
        let expected_name = ShopName::from("Test Shop");
        record.shop_name = Some(expected_name.clone());

        let result = resolve_shop_name(&record);
        assert_eq!(result, expected_name);
    }

    #[tokio::test]
    async fn should_approve_new_shop_application_when_valid_for_full_approve_path() {
        let partner_application_id = PartnerShopApplicationId::new();
        let applicant_user_id = UserId::new();
        let shop_name = ShopName::from("My New Shop");

        let mut record: PartnerShopApplicationRecord = Faker.fake();
        record.id = partner_application_id;
        record.applicant_user_id = applicant_user_id;
        record.payload_type = PartnerShopApplicationPayloadTypeRecord::New;
        record.shop_name = Some(shop_name.clone());
        record.shop_type = Some(ShopTypeRecord::CommercialDealer);
        record.shop_domains = Some(Default::default());
        record.shop_image = None;

        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .withf(move |id| *id == partner_application_id)
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        mock_repo
            .expect_update_partner_shop_application_record()
            .withf(|_user_id, _id, update| {
                update.business_state == Some(PartnerShopApplicationStateRecord::Approved)
            })
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let created_shop: Shop = Faker.fake();
        let created_shop_id = created_shop.shop_id;
        let mut mock_shop_service = MockCommandShopService::new();
        mock_shop_service
            .expect_create()
            .withf(move |cmd| cmd.name == ShopName::from("My New Shop"))
            .return_once(move |_| Box::pin(async move { Ok(created_shop) }));

        let mut mock_shop_repo = MockShopDynamoDbRepository::new();
        mock_shop_repo
            .expect_update_shop_record()
            .withf(move |shop_id, update| {
                *shop_id == created_shop_id
                    && update.partner_user_id.is_some()
                    && update.gsi1_pk.is_some()
                    && update.gsi1_sk.is_some()
            })
            .returning(|_, _| Box::pin(async { Ok(None) }));

        let mut mock_notification = MockNotificationService::new();
        mock_notification
            .expect_create_notification()
            .withf(move |_event_id, cmd| {
                cmd.user_id == applicant_user_id
                    && cmd.external
                    && matches!(
                        &cmd.notification_payload,
                        NotificationPayload::PartnerApplication {
                            partner_application_payload:
                                NotificationPartnerApplicationPayload::Approved { .. },
                            ..
                        }
                    )
            })
            .return_once(|_, cmd| {
                let notification = fake_notification(cmd.user_id);
                Box::pin(async move { Ok(notification) })
            });

        let input = StepFunctionInput {
            step: StepFunctionStep::Approve,
            task_token: None,
            partner_application_id,
            applicant_user_id,
        };

        let result = handle_approve(
            &mock_repo,
            &mock_shop_service,
            &mock_shop_repo,
            &mock_notification,
            &input,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_approve_existing_shop_application_when_valid_for_full_approve_path() {
        let partner_application_id = PartnerShopApplicationId::new();
        let applicant_user_id = UserId::new();
        let existing_shop_id: ShopId = Faker.fake();

        let mut record: PartnerShopApplicationRecord = Faker.fake();
        record.id = partner_application_id;
        record.applicant_user_id = applicant_user_id;
        record.payload_type = PartnerShopApplicationPayloadTypeRecord::Existing;
        record.existing_shop_id = Some(existing_shop_id);

        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        mock_repo
            .expect_update_partner_shop_application_record()
            .withf(|_user_id, _id, update| {
                update.business_state == Some(PartnerShopApplicationStateRecord::Approved)
            })
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let mock_shop_service = MockCommandShopService::new();

        let mut shop_record: ShopRecord = Faker.fake();
        shop_record.shop_id = existing_shop_id;
        shop_record.partner_user_id = None;

        let mut mock_shop_repo = MockShopDynamoDbRepository::new();
        mock_shop_repo
            .expect_get_shop_record()
            .withf(move |id| *id == existing_shop_id)
            .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));

        mock_shop_repo
            .expect_update_shop_record()
            .withf(move |shop_id, update| {
                *shop_id == existing_shop_id
                    && update.partner_user_id == Some(applicant_user_id)
                    && update.gsi1_pk.is_some()
                    && update.gsi1_sk.is_some()
            })
            .returning(|_, _| Box::pin(async { Ok(None) }));

        let mut mock_notification = MockNotificationService::new();
        mock_notification
            .expect_create_notification()
            .withf(move |_event_id, cmd| {
                cmd.user_id == applicant_user_id
                    && matches!(
                        &cmd.notification_payload,
                        NotificationPayload::PartnerApplication {
                            partner_application_payload:
                                NotificationPartnerApplicationPayload::Approved { .. },
                            ..
                        }
                    )
            })
            .return_once(|_, cmd| {
                let notification = fake_notification(cmd.user_id);
                Box::pin(async move { Ok(notification) })
            });

        let input = StepFunctionInput {
            step: StepFunctionStep::Approve,
            task_token: None,
            partner_application_id,
            applicant_user_id,
        };

        let result = handle_approve(
            &mock_repo,
            &mock_shop_service,
            &mock_shop_repo,
            &mock_notification,
            &input,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_reject_application_when_valid_for_full_reject_path() {
        let partner_application_id = PartnerShopApplicationId::new();
        let applicant_user_id = UserId::new();
        let shop_name = ShopName::from("Test Shop");

        let mut record: PartnerShopApplicationRecord = Faker.fake();
        record.id = partner_application_id;
        record.applicant_user_id = applicant_user_id;
        record.shop_name = Some(shop_name.clone());

        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        mock_repo
            .expect_update_partner_shop_application_record()
            .withf(|_user_id, _id, update| {
                update.business_state == Some(PartnerShopApplicationStateRecord::Rejected)
            })
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let mut mock_notification = MockNotificationService::new();
        mock_notification
            .expect_create_notification()
            .withf(move |_event_id, cmd| {
                cmd.user_id == applicant_user_id
                    && cmd.external
                    && matches!(
                        &cmd.notification_payload,
                        NotificationPayload::PartnerApplication {
                            partner_application_payload:
                                NotificationPartnerApplicationPayload::Rejected { .. },
                            ..
                        }
                    )
            })
            .return_once(|_, cmd| {
                let notification = fake_notification(cmd.user_id);
                Box::pin(async move { Ok(notification) })
            });

        let input = StepFunctionInput {
            step: StepFunctionStep::Reject,
            task_token: None,
            partner_application_id,
            applicant_user_id,
        };

        let result = handle_reject(&mock_repo, &mock_notification, &input).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_fail_approve_when_shop_creation_fails_for_new_application() {
        let partner_application_id = PartnerShopApplicationId::new();
        let applicant_user_id = UserId::new();

        let mut record: PartnerShopApplicationRecord = Faker.fake();
        record.id = partner_application_id;
        record.applicant_user_id = applicant_user_id;
        record.payload_type = PartnerShopApplicationPayloadTypeRecord::New;
        record.shop_name = Some(ShopName::from("Failing Shop"));
        record.shop_type = Some(ShopTypeRecord::Marketplace);

        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        let mut mock_shop_service = MockCommandShopService::new();
        mock_shop_service.expect_create().return_once(|_| {
            use shop::service::command_service::CommandShopError;
            Box::pin(async {
                Err(CommandShopError::ShopSlugExistsAlready(
                    ShopName::from("Failing Shop"),
                    common::slug_id::SlugId::from("failing-shop"),
                ))
            })
        });

        let mock_shop_repo = MockShopDynamoDbRepository::new();
        let mock_notification = MockNotificationService::new();

        let input = StepFunctionInput {
            step: StepFunctionStep::Approve,
            task_token: None,
            partner_application_id,
            applicant_user_id,
        };

        let result = handle_approve(
            &mock_repo,
            &mock_shop_service,
            &mock_shop_repo,
            &mock_notification,
            &input,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StepFunctionError::ShopCreationError(_)
        ));
    }

    #[tokio::test]
    async fn should_fail_approve_when_shop_name_missing_for_new_application() {
        let partner_application_id = PartnerShopApplicationId::new();
        let applicant_user_id = UserId::new();

        let mut record: PartnerShopApplicationRecord = Faker.fake();
        record.id = partner_application_id;
        record.applicant_user_id = applicant_user_id;
        record.payload_type = PartnerShopApplicationPayloadTypeRecord::New;
        record.shop_name = None;
        record.shop_type = Some(ShopTypeRecord::CommercialDealer);

        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        let mock_shop_service = MockCommandShopService::new();
        let mock_shop_repo = MockShopDynamoDbRepository::new();
        let mock_notification = MockNotificationService::new();

        let input = StepFunctionInput {
            step: StepFunctionStep::Approve,
            task_token: None,
            partner_application_id,
            applicant_user_id,
        };

        let result = handle_approve(
            &mock_repo,
            &mock_shop_service,
            &mock_shop_repo,
            &mock_notification,
            &input,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StepFunctionError::MissingField(_)
        ));
    }

    #[tokio::test]
    async fn should_fail_approve_when_existing_shop_id_missing_for_existing_application() {
        let partner_application_id = PartnerShopApplicationId::new();
        let applicant_user_id = UserId::new();

        let mut record: PartnerShopApplicationRecord = Faker.fake();
        record.id = partner_application_id;
        record.applicant_user_id = applicant_user_id;
        record.payload_type = PartnerShopApplicationPayloadTypeRecord::Existing;
        record.existing_shop_id = None;

        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_query_partner_shop_application_record_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        let mock_shop_service = MockCommandShopService::new();
        let mock_shop_repo = MockShopDynamoDbRepository::new();
        let mock_notification = MockNotificationService::new();

        let input = StepFunctionInput {
            step: StepFunctionStep::Approve,
            task_token: None,
            partner_application_id,
            applicant_user_id,
        };

        let result = handle_approve(
            &mock_repo,
            &mock_shop_service,
            &mock_shop_repo,
            &mock_notification,
            &input,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StepFunctionError::MissingField(_)
        ));
    }

    #[tokio::test]
    async fn should_handle_wait_for_review_step_when_valid_for_full_handler() {
        let partner_application_id = PartnerShopApplicationId::new();
        let applicant_user_id = UserId::new();

        let payload = serde_json::json!({
            "step": "WAIT_FOR_REVIEW",
            "task_token": "my-task-token",
            "partner_application_id": partner_application_id.to_string(),
            "applicant_user_id": applicant_user_id.to_string(),
        });

        let mut mock_repo = MockPartnerShopApplicationDynamoDbRepository::new();
        mock_repo
            .expect_update_partner_shop_application_record()
            .withf(|_user_id, _id, update| {
                update.business_state == Some(PartnerShopApplicationStateRecord::InReview)
                    && update.task_token == Some("my-task-token".to_string())
            })
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let mock_shop_service = MockCommandShopService::new();
        let mock_shop_repo = MockShopDynamoDbRepository::new();
        let mock_notification = MockNotificationService::new();

        let event = lambda_runtime::LambdaEvent::new(payload, lambda_runtime::Context::default());

        let result = handler(
            &mock_repo,
            &mock_shop_service,
            &mock_shop_repo,
            &mock_notification,
            event,
        )
        .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(
            output
                .get("partner_application_id")
                .unwrap()
                .as_str()
                .unwrap(),
            partner_application_id.to_string()
        );
        assert_eq!(
            output.get("applicant_user_id").unwrap().as_str().unwrap(),
            applicant_user_id.to_string()
        );
    }
}
