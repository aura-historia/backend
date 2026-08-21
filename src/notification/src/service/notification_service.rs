use crate::core::mail_template::{MailTemplate, MailTemplateType};
use crate::service::{s3_adapter::S3Adapter, ses_adapter::SesAdapter};
use crate::{
    core::{
        notification::{
            LocalizedNotification, Notification, NotificationPartnerApplicationPayload,
            NotificationPayload, NotificationWatchlistPayload,
        },
        notification_id::NotificationId,
    },
    dynamodb::{
        notification_record::NotificationRecord,
        notification_record_update::NotificationRecordUpdate,
        notification_type_record::NotificationTypeRecord,
        repository::NotificationDynamoDbRepository,
    },
    service::command::{CreateNotificationCommand, UpdateNotificationCommand},
};
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_sesv2::{
    operation::send_email::SendEmailError,
    types::{Body, Content, EmailContent, Message, MessageTag},
};
use common::{
    actor::RequestContext,
    batch::Batch,
    currency::domain::Currency,
    event_id::EventId,
    language::domain::Language,
    pagination::cursor::{Cursor, CursoredResult},
    product_id::ProductId,
    user_id::UserId,
};
use handlebars::Handlebars;
use once_cell::sync::OnceCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use user::service::user_service::{UserService, UserServiceError};

const SENDER_MAIL: &str = "no-reply@notify.aura-historia.com";
const REPLY_TO_MAIL: &str = "contact@aura-historia.com";

#[derive(thiserror::Error, Debug)]
pub enum NotificationError {
    #[error("There exists no Notification for user '{0}' with origin-event-id '{1}'.")]
    NotificationNotFound(UserId, EventId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(
        #[source] Box<SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0:?}")]
    SdkQueryError(
        #[source] Box<SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>>,
    ),

    #[error("Encountered DynamoDB SdkError for PutItem: {0:?}")]
    SdkPutItemError(
        #[source] Box<SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>>,
    ),

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0:?}")]
    SdkUpdateItemError(
        #[source]
        Box<SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>>,
    ),

    #[error("Encountered DynamoDB SdkError for DeleteItem: {0:?}")]
    SdkDeleteItemError(
        #[source]
        Box<SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError, HttpResponse>>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchWriteItem (delete): {0}")]
    SdkBatchDeleteError(
        #[source]
        Box<
            SdkError<
                aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemError,
                HttpResponse,
            >,
        >,
    ),

    #[error("User with UserId '{0}' not found.")]
    UserNotFound(UserId),

    #[error("Failed looking up user: {0}")]
    UserLookupFailed(#[source] Box<UserServiceError>),

    #[error("Encountered SES SdkError for SendMail: {0:?}")]
    SdkSESSendMailError(#[source] Box<SdkError<SendEmailError>>),

    #[error("Encountered S3 SdkError for GetObject: {0:?}")]
    SdkS3GetObjectError(#[source] Box<SdkError<GetObjectError>>),

    #[error("Encountered Handlebars-Error for Render: {0}")]
    TemplateRenderError(#[from] handlebars::RenderError),

    #[error("Missing persistence field: {0}")]
    MissingPersistenceField(#[from] common::error::missing_field::MissingPersistenceField),
}

impl From<SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>>
    for NotificationError
{
    fn from(
        error: SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ) -> Self {
        Self::SdkGetItemError(Box::new(error))
    }
}

impl From<SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>>
    for NotificationError
{
    fn from(error: SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>) -> Self {
        Self::SdkQueryError(Box::new(error))
    }
}

impl From<SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>>
    for NotificationError
{
    fn from(
        error: SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ) -> Self {
        Self::SdkPutItemError(Box::new(error))
    }
}

impl From<SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>>
    for NotificationError
{
    fn from(
        error: SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>,
    ) -> Self {
        Self::SdkUpdateItemError(Box::new(error))
    }
}

impl From<SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError, HttpResponse>>
    for NotificationError
{
    fn from(
        error: SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError, HttpResponse>,
    ) -> Self {
        Self::SdkDeleteItemError(Box::new(error))
    }
}

impl
    From<SdkError<aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemError, HttpResponse>>
    for NotificationError
{
    fn from(
        error: SdkError<
            aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemError,
            HttpResponse,
        >,
    ) -> Self {
        Self::SdkBatchDeleteError(Box::new(error))
    }
}

#[derive(Debug)]
pub struct CreateNotificationsResult {
    pub unprocessed: Vec<(CreateNotificationCommand, NotificationError)>,
    pub processed: Vec<Notification>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait NotificationService {
    async fn find_notification(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError>;

    async fn create_notification(
        &self,
        ctx: &RequestContext,
        origin_event_id: &EventId,
        cmd: CreateNotificationCommand,
    ) -> Result<Notification, NotificationError>;

    async fn create_notifications(
        &self,
        ctx: &RequestContext,
        origin_event_id: &EventId,
        cmds: Vec<CreateNotificationCommand>,
    ) -> CreateNotificationsResult;

    async fn update_notification(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        origin_event_id: &EventId,
        update: UpdateNotificationCommand,
    ) -> Result<Notification, NotificationError>;

    async fn view_notifications(
        &self,
        user_id: &UserId,
        languages: &[Language],
        currency: &Currency,
        cursor: &Option<Cursor<EventId>>,
    ) -> Result<CursoredResult<LocalizedNotification, EventId>, NotificationError>;

    async fn send_externally(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError>;

    async fn update_notifications(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        cmd: UpdateNotificationCommand,
    ) -> Result<CursoredResult<Notification, EventId>, NotificationError>;

    async fn delete_notification(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<(), NotificationError>;

    async fn delete_notifications(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
    ) -> Result<(), NotificationError>;

    async fn find_notifications_by_product(
        &self,
        user_id: &UserId,
        product_id: &ProductId,
        limit: Option<i32>,
        scan_index_forward: bool,
    ) -> Result<Vec<Notification>, NotificationError>;
}

static TEMPLATE_CACHE: OnceCell<Arc<RwLock<HashMap<MailTemplate, String>>>> = OnceCell::new();

pub struct NotificationServiceImpl<'a> {
    notification_repository: &'a (dyn NotificationDynamoDbRepository + Sync),
    user_service: &'a (dyn UserService + Sync),
    ses_adapter: &'a (dyn SesAdapter + Send + Sync),
    s3_adapter: &'a (dyn S3Adapter + Send + Sync),
    s3_bucket: &'a str,
    stage_name: &'a str,
    commit_sha: &'a str,
    handlebars: Handlebars<'a>,
}

impl<'a> NotificationServiceImpl<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        notification_repository: &'a (dyn NotificationDynamoDbRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
        ses_adapter: &'a (dyn SesAdapter + Send + Sync),
        s3_adapter: &'a (dyn S3Adapter + Send + Sync),
        s3_bucket: &'a str,
        stage_name: &'a str,
        commit_sha: &'a str,
    ) -> Self {
        Self {
            notification_repository,
            user_service,
            ses_adapter,
            s3_adapter,
            s3_bucket,
            stage_name,
            commit_sha,
            handlebars: Handlebars::new(),
        }
    }

    #[allow(clippy::result_large_err)]
    async fn resolve_template(
        &self,
        template: MailTemplate,
    ) -> Result<String, Box<SdkError<GetObjectError>>> {
        let template_cache_rw =
            TEMPLATE_CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())));

        {
            let template_cache_r = template_cache_rw.read().await;
            if let Some(resolved) = template_cache_r.get(&template) {
                return Ok(resolved.clone());
            }
        }

        let s3_key = format!(
            "{}/{}/{}.html",
            self.stage_name,
            self.commit_sha,
            template.as_s3_blob_str()
        );
        let resp = self.s3_adapter.get_object(self.s3_bucket, &s3_key).await?;
        let bytes = resp
            .body
            .collect()
            .await
            .map_err(SdkError::construction_failure)?
            .into_bytes();
        let template_html = String::from_utf8_lossy(&bytes).to_string();

        {
            let mut template_cache_w = template_cache_rw.write().await;
            template_cache_w.insert(template, template_html.clone());
        }

        Ok(template_html)
    }

    #[allow(clippy::result_large_err)]
    async fn send_notification_as_email(
        &self,
        notification: &Notification,
    ) -> Result<(), NotificationError> {
        let user = self
            .user_service
            .find_user(&notification.user_id)
            .await
            .map_err(|err| match &err {
                UserServiceError::UserNotFound(uid) => NotificationError::UserNotFound(*uid),
                _ => NotificationError::UserLookupFailed(Box::new(err)),
            })?;

        let language = user.language.unwrap_or(Language::En);
        let currency = user.currency.unwrap_or(Currency::Eur);

        let mail_template = derive_mail_template(&notification.notification_payload, &language);
        let subject = build_email_subject(&notification.notification_payload, &language);
        let template_data =
            build_email_template_data(notification, &language, &currency, user.first_name.as_ref());

        let template_html = self
            .resolve_template(mail_template)
            .await
            .map_err(NotificationError::SdkS3GetObjectError)?;

        let rendered_html = self
            .handlebars
            .render_template(&template_html, &template_data)?;

        let subject_content = Content::builder()
            .data(subject)
            .build()
            .expect("shouldn't fail because 'data' was set explicitly");
        let body = Body::builder()
            .html(
                Content::builder()
                    .data(rendered_html)
                    .build()
                    .expect("shouldn't fail because 'data' was set explicitly"),
            )
            .build();
        let message = Message::builder()
            .subject(subject_content)
            .body(body)
            .build();
        let content = EmailContent::builder().simple(message).build();

        let message_tag = MessageTag::builder()
            .name("template_type")
            .value(mail_template.template_type.as_message_tag_value())
            .build()
            .expect("shouldn't fail because 'name' and 'value' were set explicitly");

        self.ses_adapter
            .send_email(
                SENDER_MAIL.try_into().expect("valid sender email"),
                user.email,
                REPLY_TO_MAIL.try_into().expect("valid reply-to email"),
                content,
                vec![message_tag],
            )
            .await
            .map_err(|error| NotificationError::SdkSESSendMailError(Box::new(error)))?;

        Ok(())
    }
}

fn derive_mail_template(payload: &NotificationPayload, language: &Language) -> MailTemplate {
    let template_type = match payload {
        NotificationPayload::Watchlist {
            watchlist_payload, ..
        } => match watchlist_payload {
            NotificationWatchlistPayload::PriceChange { .. } => {
                MailTemplateType::WatchlistUpdatePrice
            }
            NotificationWatchlistPayload::StateChange { .. } => {
                MailTemplateType::WatchlistUpdateState
            }
        },
        NotificationPayload::SearchFilter { .. } => MailTemplateType::SearchFilterMatch,
        NotificationPayload::PartnerApplication {
            partner_application_payload,
            ..
        } => match partner_application_payload {
            NotificationPartnerApplicationPayload::Approved { .. } => {
                MailTemplateType::PartnerApplicationApproval
            }
            NotificationPartnerApplicationPayload::Rejected { .. } => {
                MailTemplateType::PartnerApplicationRejection
            }
        },
    };
    MailTemplate {
        template_type,
        language: (*language).into(),
    }
}

fn build_email_subject(payload: &NotificationPayload, language: &Language) -> String {
    match payload {
        NotificationPayload::Watchlist {
            title,
            watchlist_payload,
            ..
        } => {
            let resolved_title = Language::resolve(&[*language], title.clone())
                .map(|l| l.payload.to_string())
                .unwrap_or_else(|| "Unknown".to_owned());

            match watchlist_payload {
                NotificationWatchlistPayload::PriceChange { .. } => match language {
                    Language::De => format!("Preisänderung: {resolved_title}"),
                    Language::Fr => format!("Changement de prix : {resolved_title}"),
                    Language::Es => format!("Cambio de precio: {resolved_title}"),
                    Language::It => format!("Variazione di prezzo: {resolved_title}"),
                    _ => format!("Price change: {resolved_title}"),
                },
                NotificationWatchlistPayload::StateChange { new_state, .. } => {
                    let state_str = new_state.format_human_readable(language);
                    match language {
                        Language::De => {
                            format!("Statusänderung ({state_str}): {resolved_title}")
                        }
                        Language::Fr => {
                            format!("Changement de statut ({state_str}) : {resolved_title}")
                        }
                        Language::Es => {
                            format!("Cambio de estado ({state_str}): {resolved_title}")
                        }
                        Language::It => {
                            format!("Cambio di stato ({state_str}): {resolved_title}")
                        }
                        _ => format!("Status change ({state_str}): {resolved_title}"),
                    }
                }
            }
        }
        NotificationPayload::SearchFilter {
            title,
            search_filter_payload,
            ..
        } => {
            let resolved_title = Language::resolve(&[*language], title.clone())
                .map(|l| l.payload.to_string())
                .unwrap_or_else(|| "Unknown".to_owned());
            let filter_name = &search_filter_payload.user_search_filter_name;
            match language {
                Language::De => {
                    format!("Neues Ergebnis für \"{filter_name}\": {resolved_title}")
                }
                Language::Fr => {
                    format!("Nouveau résultat pour \"{filter_name}\" : {resolved_title}")
                }
                Language::Es => {
                    format!("Nuevo resultado para \"{filter_name}\": {resolved_title}")
                }
                Language::It => {
                    format!("Nuovo risultato per \"{filter_name}\": {resolved_title}")
                }
                _ => format!("New match for \"{filter_name}\": {resolved_title}"),
            }
        }
        NotificationPayload::PartnerApplication {
            shop_name,
            partner_application_payload,
            ..
        } => match partner_application_payload {
            NotificationPartnerApplicationPayload::Approved { .. } => match language {
                Language::De => format!("Partnerantrag genehmigt: {shop_name}"),
                Language::Fr => format!("Demande de partenariat approuvée : {shop_name}"),
                Language::Es => format!("Solicitud de asociación aprobada: {shop_name}"),
                Language::It => format!("Richiesta di partnership approvata: {shop_name}"),
                _ => format!("Partner application approved: {shop_name}"),
            },
            NotificationPartnerApplicationPayload::Rejected { .. } => match language {
                Language::De => format!("Partnerantrag abgelehnt: {shop_name}"),
                Language::Fr => format!("Demande de partenariat refusée : {shop_name}"),
                Language::Es => format!("Solicitud de asociación rechazada: {shop_name}"),
                Language::It => format!("Richiesta di partnership rifiutata: {shop_name}"),
                _ => format!("Partner application rejected: {shop_name}"),
            },
        },
    }
}

fn build_email_template_data(
    notification: &Notification,
    language: &Language,
    currency: &Currency,
    user_first_name: Option<&user::core::first_name::FirstName>,
) -> serde_json::Value {
    match &notification.notification_payload {
        NotificationPayload::Watchlist {
            product_id,
            shop_id,
            shops_product_id,
            shop_slug_id,
            product_slug_id,
            shop_name,
            title,
            image,
            url,
            view_url,
            watchlist_payload,
            ..
        } => {
            let resolved_title = Language::resolve(&[*language], title.clone())
                .map(|l| l.payload.to_string())
                .unwrap_or_else(|| "Unknown".to_owned());

            let mut data = serde_json::json!({
                "product_id": product_id.to_string(),
                "shop_id": shop_id.to_string(),
                "shops_product_id": shops_product_id.to_string(),
                "shop_slug_id": shop_slug_id.to_string(),
                "product_slug_id": product_slug_id.to_string(),
                "shop_name": shop_name.to_string(),
                "title": resolved_title,
                "language": format!("{language:?}"),
            });

            data["url"] = serde_json::json!(url.as_str());
            data["view_url"] = serde_json::json!(view_url.as_str());

            if let Some(image) = image {
                data["image_url"] = serde_json::json!(image.url.as_str());
            }

            if let Some(first_name) = user_first_name {
                data["user_first_name"] = serde_json::json!(first_name.to_string());
            }

            match watchlist_payload {
                NotificationWatchlistPayload::PriceChange {
                    old_price,
                    new_price,
                } => {
                    let old = Currency::resolve(&[*currency], old_price.clone());
                    let new = Currency::resolve(&[*currency], new_price.clone());

                    if let Some(p) = &old {
                        data["old_price"] = serde_json::json!(p.format_human_readable());
                    }
                    if let Some(p) = &new {
                        data["new_price"] = serde_json::json!(p.format_human_readable());
                    }
                    data["notification_type"] = serde_json::json!("price_change");
                }
                NotificationWatchlistPayload::StateChange {
                    old_state,
                    new_state,
                } => {
                    data["old_state"] =
                        serde_json::json!(old_state.format_human_readable(language));
                    data["new_state"] =
                        serde_json::json!(new_state.format_human_readable(language));
                    data["notification_type"] = serde_json::json!("state_change");
                }
            }

            data
        }
        NotificationPayload::SearchFilter {
            product_id,
            shop_id,
            shops_product_id,
            shop_slug_id,
            product_slug_id,
            shop_name,
            title,
            image,
            view_url,
            search_filter_payload,
            ..
        } => {
            let resolved_title = Language::resolve(&[*language], title.clone())
                .map(|l| l.payload.to_string())
                .unwrap_or_else(|| "Unknown".to_owned());
            let mut data = serde_json::json!({
                "product_id": product_id.to_string(),
                "shop_id": shop_id.to_string(),
                "shops_product_id": shops_product_id.to_string(),
                "shop_slug_id": shop_slug_id.to_string(),
                "product_slug_id": product_slug_id.to_string(),
                "shop_name": shop_name.to_string(),
                "title": resolved_title,
                "language": format!("{language:?}"),
                "notification_type": "search_filter_match",
                "search_filter_id": search_filter_payload.user_search_filter_id.to_string(),
                "search_filter_name": search_filter_payload.user_search_filter_name.to_string(),
            });

            data["view_url"] = serde_json::json!(view_url.as_str());

            if let Some(image) = image {
                data["image_url"] = serde_json::json!(image.url.as_str());
            }

            if let Some(first_name) = user_first_name {
                data["user_first_name"] = serde_json::json!(first_name.to_string());
            }

            data
        }
        NotificationPayload::PartnerApplication {
            shop_name,
            image,
            partner_application_payload,
        } => {
            let (notification_type, partner_application_id) = match partner_application_payload {
                NotificationPartnerApplicationPayload::Approved {
                    partner_application_id,
                } => ("partner_application_approval", partner_application_id),
                NotificationPartnerApplicationPayload::Rejected {
                    partner_application_id,
                } => ("partner_application_rejection", partner_application_id),
            };
            let mut data = serde_json::json!({
                "shop_name": shop_name.to_string(),
                "language": format!("{language:?}"),
                "notification_type": notification_type,
                "partner_application_id": partner_application_id.to_string(),
            });

            if let Some(first_name) = user_first_name {
                data["user_first_name"] = serde_json::json!(first_name.to_string());
            }

            if let Some(image) = image {
                data["image_url"] = serde_json::json!(image.as_str());
            }

            data
        }
    }
}

#[async_trait::async_trait]
impl<'a> NotificationService for NotificationServiceImpl<'a> {
    async fn find_notification(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError> {
        let record = self
            .notification_repository
            .get_notification_record(user_id, origin_event_id)
            .await?
            .ok_or(NotificationError::NotificationNotFound(
                *user_id,
                *origin_event_id,
            ))?;

        Ok(record.try_into()?)
    }

    async fn create_notification(
        &self,
        ctx: &RequestContext,
        origin_event_id: &EventId,
        cmd: CreateNotificationCommand,
    ) -> Result<Notification, NotificationError> {
        let now = OffsetDateTime::now_utc();
        let notification = Notification {
            user_id: cmd.user_id,
            origin_event_id: *origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: cmd.notification_payload,
            seen: false,
            external: cmd.external,
            created_by: ctx.actor,
            updated_by: ctx.actor,
            created: now,
            updated: now,
        };
        let record = NotificationRecord::from(notification.clone());
        self.notification_repository
            .put_notification_record(record)
            .await?;

        Ok(notification)
    }

    async fn create_notifications(
        &self,
        ctx: &RequestContext,
        origin_event_id: &EventId,
        cmds: Vec<CreateNotificationCommand>,
    ) -> CreateNotificationsResult {
        if cmds.is_empty() {
            return CreateNotificationsResult {
                unprocessed: vec![],
                processed: vec![],
            };
        }

        let now = OffsetDateTime::now_utc();

        // Build (cmd, notification) pairs and keep a record clone for batching.
        let pairs: Vec<(CreateNotificationCommand, Notification)> = cmds
            .into_iter()
            .map(|cmd| {
                let notification = Notification {
                    user_id: cmd.user_id,
                    origin_event_id: *origin_event_id,
                    notification_id: NotificationId::new(),
                    notification_type: None,
                    notification_payload: cmd.notification_payload.clone(),
                    seen: false,
                    external: cmd.external,
                    created_by: ctx.actor,
                    updated_by: ctx.actor,
                    created: now,
                    updated: now,
                };
                (cmd, notification)
            })
            .collect();

        // Index by user_id so we can look up cmd/notification after batch responses.
        let mut cmd_map: HashMap<UserId, (CreateNotificationCommand, Notification)> = pairs
            .into_iter()
            .map(|(cmd, notif)| (notif.user_id, (cmd, notif)))
            .collect();

        let records: Vec<NotificationRecord> = cmd_map
            .values()
            .map(|(_, notif)| NotificationRecord::from(notif.clone()))
            .collect();

        let mut processed = Vec::new();
        let mut unprocessed = Vec::new();

        let batches = Batch::<NotificationRecord, 25>::chunked_from(records.into_iter());
        for batch in batches {
            let user_ids_in_batch: Vec<UserId> = batch.iter().map(|r| r.user_id).collect();

            match self
                .notification_repository
                .put_notification_records(batch)
                .await
            {
                Ok(output) => {
                    let failed_user_ids: HashSet<UserId> = output
                        .unprocessed_items
                        .unwrap_or_default()
                        .into_values()
                        .flatten()
                        .filter_map(|req| req.put_request)
                        .filter_map(|put| {
                            match serde_dynamo::from_item::<_, NotificationRecord>(put.item) {
                                Ok(record) => Some(record.user_id),
                                Err(err) => {
                                    error!(
                                        error = ?err,
                                        r#type = std::any::type_name::<NotificationRecord>(),
                                        "Failed parsing unprocessed item from BatchWriteItem output"
                                    );
                                    None
                                }
                            }
                        })
                        .collect();

                    for user_id in user_ids_in_batch {
                        if let Some((cmd, notif)) = cmd_map.remove(&user_id) {
                            if failed_user_ids.contains(&user_id) {
                                unprocessed.push((
                                    cmd,
                                    NotificationError::SdkPutItemError(Box::new(
                                        SdkError::construction_failure(
                                            "Item returned as unprocessed in BatchWriteItem",
                                        ),
                                    )),
                                ));
                            } else {
                                processed.push(notif);
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        actor = %ctx.actor,
                        error = ?err,
                        "Failed writing NotificationRecord batch due to SdkError."
                    );
                    for user_id in user_ids_in_batch {
                        if let Some((cmd, _)) = cmd_map.remove(&user_id) {
                            unprocessed.push((
                                cmd,
                                NotificationError::SdkPutItemError(Box::new(
                                    SdkError::construction_failure(
                                        "BatchWriteItem operation failed",
                                    ),
                                )),
                            ));
                        }
                    }
                }
            }
        }

        CreateNotificationsResult {
            unprocessed,
            processed,
        }
    }

    async fn update_notification(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        origin_event_id: &EventId,
        update: UpdateNotificationCommand,
    ) -> Result<Notification, NotificationError> {
        let existing_record = self
            .notification_repository
            .get_notification_record(user_id, origin_event_id)
            .await?
            .ok_or(NotificationError::NotificationNotFound(
                *user_id,
                *origin_event_id,
            ))?;

        if update.is_empty() {
            Ok(existing_record.try_into()?)
        } else {
            let record_update = NotificationRecordUpdate {
                seen: update.seen,
                notification_type: None,
                updated_by: ctx.actor.into(),
                updated: OffsetDateTime::now_utc(),
            };

            let updated_record = self
                .notification_repository
                .update_notification_record(user_id, origin_event_id, record_update)
                .await?
                .ok_or_else(|| {
                    NotificationError::SdkUpdateItemError(Box::new(SdkError::construction_failure(
                        "Failed parsing DynamoDB UpdateItem Response-Payload",
                    )))
                })?;

            Ok(updated_record.try_into()?)
        }
    }

    async fn view_notifications(
        &self,
        user_id: &UserId,
        languages: &[Language],
        currency: &Currency,
        cursor: &Option<Cursor<EventId>>,
    ) -> Result<CursoredResult<LocalizedNotification, EventId>, NotificationError> {
        let cursor = (*cursor).unwrap_or_default();
        let scan_index_forward = false; // newest first

        let paged_records = self
            .notification_repository
            .query_notification_records(user_id, &cursor, scan_index_forward)
            .await?;
        let last = paged_records.last().cloned();

        let notifications: Vec<LocalizedNotification> = paged_records
            .into_iter()
            .map(Notification::try_from)
            .filter_map(Result::ok)
            .map(|n| n.localized(currency, languages))
            .collect();

        let total = if notifications.is_empty() {
            0
        } else {
            self.notification_repository
                .count_notification_records(user_id, &cursor, scan_index_forward)
                .await?
        };

        Ok(CursoredResult {
            cursor: Cursor {
                size: notifications.len() as u64,
                search_after: last.map(|l| l.origin_event_id),
            },
            items: notifications,
            total: Some(total),
        })
    }

    async fn send_externally(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError> {
        let record = self
            .notification_repository
            .get_notification_record(user_id, origin_event_id)
            .await?
            .ok_or(NotificationError::NotificationNotFound(
                *user_id,
                *origin_event_id,
            ))?;

        let notification: Notification = record.try_into()?;

        // Idempotency: if already sent externally, return as-is.
        if notification.notification_type.is_some() {
            info!(
                actor = %ctx.actor,
                userId = %user_id,
                originEventId = %origin_event_id,
                notificationType = ?notification.notification_type,
                "Notification has already been sent externally. Skipping."
            );
            return Ok(notification);
        }

        // Only send externally if the user opted in.
        if !notification.external {
            debug!(
                actor = %ctx.actor,
                userId = %user_id,
                originEventId = %origin_event_id,
                "Notification has external=false. Skipping external send."
            );
            return Ok(notification);
        }

        // Currently always send as email. Later the external type/target will
        // be determined by user-preferences.
        self.send_notification_as_email(&notification).await?;

        // Persist that the notification was sent as email.
        let record_update = NotificationRecordUpdate {
            seen: None,
            notification_type: Some(NotificationTypeRecord::Email),
            updated_by: ctx.actor.into(),
            updated: OffsetDateTime::now_utc(),
        };

        let updated_record = self
            .notification_repository
            .update_notification_record(user_id, origin_event_id, record_update)
            .await?
            .ok_or_else(|| {
                NotificationError::SdkUpdateItemError(Box::new(SdkError::construction_failure(
                    "Failed parsing DynamoDB UpdateItem Response-Payload after send_externally",
                )))
            })?;

        let updated_notification: Notification = updated_record.try_into()?;
        info!(
            actor = %ctx.actor,
            userId = %user_id,
            originEventId = %origin_event_id,
            "Notification sent externally as email and persisted."
        );

        Ok(updated_notification)
    }

    async fn update_notifications(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        cmd: UpdateNotificationCommand,
    ) -> Result<CursoredResult<Notification, EventId>, NotificationError> {
        let all_records = self
            .notification_repository
            .query_all_notification_records(user_id)
            .await?;

        let record_update = NotificationRecordUpdate {
            seen: cmd.seen,
            notification_type: None,
            updated_by: ctx.actor.into(),
            updated: OffsetDateTime::now_utc(),
        };

        for record in &all_records {
            self.notification_repository
                .update_notification_record(user_id, &record.origin_event_id, record_update.clone())
                .await?;
        }

        info!(
            actor = %ctx.actor,
            userId = %user_id,
            count = all_records.len(),
            cmd = ?cmd,
            "All notifications updated."
        );

        let cursor = Cursor::default();
        let scan_index_forward = false;
        let paged_records = self
            .notification_repository
            .query_notification_records(user_id, &cursor, scan_index_forward)
            .await?;
        let last = paged_records.last().cloned();

        let notifications: Vec<Notification> = paged_records
            .into_iter()
            .filter_map(|r| Notification::try_from(r).ok())
            .collect();

        let total = if notifications.is_empty() {
            0
        } else {
            self.notification_repository
                .count_notification_records(user_id, &cursor, scan_index_forward)
                .await?
        };

        Ok(CursoredResult {
            cursor: Cursor {
                size: notifications.len() as u64,
                search_after: last.map(|l| l.origin_event_id),
            },
            items: notifications,
            total: Some(total),
        })
    }

    async fn delete_notification(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<(), NotificationError> {
        self.notification_repository
            .get_notification_record(user_id, origin_event_id)
            .await?
            .ok_or(NotificationError::NotificationNotFound(
                *user_id,
                *origin_event_id,
            ))?;

        self.notification_repository
            .delete_notification_record(user_id, origin_event_id)
            .await?;

        info!(
            actor = %ctx.actor,
            userId = %user_id,
            originEventId = %origin_event_id,
            "Notification deleted."
        );

        Ok(())
    }

    async fn delete_notifications(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
    ) -> Result<(), NotificationError> {
        let all_records = self
            .notification_repository
            .query_all_notification_records(user_id)
            .await?;

        if all_records.is_empty() {
            return Ok(());
        }

        let ids: Vec<EventId> = all_records.iter().map(|r| r.origin_event_id).collect();

        for batch in Batch::chunked_from(ids.into_iter()) {
            self.notification_repository
                .delete_notification_records(user_id, &batch)
                .await?;
        }

        info!(
            actor = %ctx.actor,
            userId = %user_id,
            count = all_records.len(),
            "All notifications deleted."
        );

        Ok(())
    }

    async fn find_notifications_by_product(
        &self,
        user_id: &UserId,
        product_id: &ProductId,
        limit: Option<i32>,
        scan_index_forward: bool,
    ) -> Result<Vec<Notification>, NotificationError> {
        let records = self
            .notification_repository
            .query_product_notification_records(user_id, product_id, limit, scan_index_forward)
            .await?;

        let notifications = records
            .into_iter()
            .filter_map(|record| match Notification::try_from(record) {
                Ok(notification) => Some(notification),
                Err(err) => {
                    error!(
                        userId = %user_id,
                        productId = %product_id,
                        error = %err,
                        "Failed converting NotificationRecord to Notification."
                    );
                    None
                }
            })
            .collect();

        Ok(notifications)
    }
}

#[cfg(feature = "data")]
mod api_error_impls {
    use super::NotificationError;
    use common::api::error::ApiError;
    use common::api::error_code::{INTERNAL_SERVER_ERROR, NOTIFICATION_NOT_FOUND};

    impl From<NotificationError> for ApiError {
        fn from(err: NotificationError) -> Self {
            match err {
                NotificationError::NotificationNotFound(_, _) => {
                    ApiError::not_found(NOTIFICATION_NOT_FOUND, Box::new(err))
                }
                NotificationError::SdkGetItemError(e) => (*e).into(),
                NotificationError::SdkQueryError(e) => (*e).into(),
                NotificationError::SdkPutItemError(e) => (*e).into(),
                NotificationError::SdkUpdateItemError(e) => (*e).into(),
                NotificationError::SdkDeleteItemError(e) => (*e).into(),
                NotificationError::SdkBatchDeleteError(e) => (*e).into(),
                NotificationError::UserNotFound(_)
                | NotificationError::UserLookupFailed(_)
                | NotificationError::SdkSESSendMailError(_)
                | NotificationError::SdkS3GetObjectError(_)
                | NotificationError::TemplateRenderError(_)
                | NotificationError::MissingPersistenceField(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{s3_adapter::MockS3Adapter, ses_adapter::MockSesAdapter};
    use crate::{
        core::{
            notification::{NotificationPayload, NotificationWatchlistPayload},
            notification_type::NotificationType,
        },
        dynamodb::notification_record::NotificationRecord,
        dynamodb::repository::MockNotificationDynamoDbRepository,
    };
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError as DynamoSdkError},
        operation::{batch_write_item::BatchWriteItemOutput, put_item::PutItemOutput},
        types::{PutRequest, WriteRequest},
    };
    use aws_sdk_sesv2::operation::send_email::SendEmailOutput;
    use common::{
        actor::domain::Actor, currency::domain::Currency, language::domain::Language,
        price::domain::MonetaryAmount, product_state::domain::ProductState, user_id::UserId,
    };
    use fake::{Fake, Faker};
    use product::core::{product_image::ProductImage, prohibited_content::ProhibitedContent};
    use std::collections::HashMap;
    use user::{core::user::User, service::user_service::MockUserService};

    fn make_service<'a>(
        repository: &'a MockNotificationDynamoDbRepository,
        user_service: &'a MockUserService,
        ses_adapter: &'a MockSesAdapter,
        s3_adapter: &'a MockS3Adapter,
    ) -> NotificationServiceImpl<'a> {
        NotificationServiceImpl::new(
            repository,
            user_service,
            ses_adapter,
            s3_adapter,
            "test-bucket",
            "test-stage",
            "test-sha",
        )
    }

    fn system_ctx() -> RequestContext {
        RequestContext {
            actor: Actor::System,
        }
    }

    fn make_user(user_id: UserId) -> User {
        User {
            user_id,
            email: "test@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: Some(Language::En),
            currency: Some(Currency::Eur),
            measurement_unit: None,
            prohibited_content_consent: false,
            tier: user::core::tier::UserTier::Free,
            role: user::core::role::UserRole::User,
            stripe_customer_id: None,
            structured_address: None,
            geo_address: None,
            partner_shops: Default::default(),
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn make_notification_record_with_type(
        user_id: UserId,
        origin_event_id: EventId,
        notification_type: Option<NotificationTypeRecord>,
    ) -> NotificationRecord {
        let mut record = Faker.fake::<NotificationRecord>();
        record.user_id = user_id;
        record.origin_event_id = origin_event_id;
        record.notification_type = notification_type;
        record.pk = crate::dynamodb::notification_record::mk_pk(&user_id);
        record.sk = crate::dynamodb::notification_record::mk_sk(&origin_event_id);
        record
    }

    mod create_notifications {
        use super::*;

        fn make_test_command() -> CreateNotificationCommand {
            CreateNotificationCommand {
                user_id: UserId::new(),
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test-product-123".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Test Shop".into(),
                    title: HashMap::from([(Language::En, "Test Title".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://prf.hn/click/camref:abc/destination:https%3A%2F%2Fexample.com%2Fitem%2F1",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                external: false,
            }
        }

        #[tokio::test]
        async fn should_return_empty_when_no_commands() {
            // No mock expectations set — any call to the repository would cause a panic.
            let repository = MockNotificationDynamoDbRepository::default();
            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service
                .create_notifications(&system_ctx(), &Faker.fake(), vec![])
                .await;

            assert!(processed.is_empty());
            assert!(unprocessed.is_empty());
        }

        #[tokio::test]
        async fn should_create_notifications_when_all_succeed() {
            let cmds: Vec<CreateNotificationCommand> =
                (0..3).map(|_| make_test_command()).collect();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_records()
                .return_once(|_| Box::pin(async { Ok(BatchWriteItemOutput::builder().build()) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service
                .create_notifications(&system_ctx(), &Faker.fake(), cmds)
                .await;

            assert_eq!(processed.len(), 3);
            assert!(unprocessed.is_empty());
        }

        #[tokio::test]
        async fn should_mark_unprocessed_when_batch_write_returns_unprocessed_items() {
            use common::batch::Batch;

            let cmd_to_fail = make_test_command();
            let failing_user_id = cmd_to_fail.user_id;
            let cmd_to_succeed = make_test_command();

            let cmds = vec![cmd_to_fail, cmd_to_succeed];

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository.expect_put_notification_records().return_once(
                move |batch: Batch<NotificationRecord, 25>| {
                    let failing_item = batch
                        .iter()
                        .find(|r| r.user_id == failing_user_id)
                        .and_then(|r| serde_dynamo::to_item(r).ok())
                        .expect("failing record must be in the batch");

                    let unprocessed_write_req = WriteRequest::builder()
                        .put_request(
                            PutRequest::builder()
                                .set_item(Some(failing_item))
                                .build()
                                .unwrap(),
                        )
                        .build();
                    let output = BatchWriteItemOutput::builder()
                        .unprocessed_items("irrelevant_table", vec![unprocessed_write_req])
                        .build();

                    Box::pin(async move { Ok(output) })
                },
            );

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service
                .create_notifications(&system_ctx(), &Faker.fake(), cmds)
                .await;

            assert_eq!(processed.len(), 1);
            assert_eq!(unprocessed.len(), 1);
            assert_eq!(unprocessed[0].0.user_id, failing_user_id);
        }

        #[tokio::test]
        async fn should_propagate_sdk_error_batch_write() {
            let cmds: Vec<CreateNotificationCommand> =
                (0..3).map(|_| make_test_command()).collect();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_records()
                .return_once(|_| {
                    Box::pin(async {
                        Err(aws_sdk_dynamodb::error::SdkError::construction_failure(
                            "Simulated BatchWriteItem failure",
                        ))
                    })
                });

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service
                .create_notifications(&system_ctx(), &Faker.fake(), cmds)
                .await;

            assert!(processed.is_empty());
            assert_eq!(unprocessed.len(), 3);
        }
    }

    mod find_notification {
        use super::*;

        #[tokio::test]
        async fn should_err_notification_not_found_when_no_notification_exists() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let user_id = UserId::new();
            let origin_event_id = EventId::new();
            let actual = service
                .find_notification(&user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::NotificationNotFound(err_user_id, err_origin_event_id) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(origin_event_id, err_origin_event_id);
                }
                err => {
                    panic!("Expected 'NotificationError::NotificationNotFound' but got '{err}'")
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let actual = service
                .find_notification(&Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkGetItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkGetItemError', got '{err}'"),
            }
        }
    }

    mod create_notification {
        use super::*;

        fn make_test_command() -> CreateNotificationCommand {
            CreateNotificationCommand {
                user_id: UserId::new(),
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test-product-123".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Test Shop".into(),
                    title: HashMap::from([(Language::En, "Test Title".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                external: false,
            }
        }

        #[tokio::test]
        async fn should_create_when_success() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .create_notification(&system_ctx(), &Faker.fake(), make_test_command())
                .await
                .unwrap();

            assert!(!result.seen);
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_put_item(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_record()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let actual = service
                .create_notification(&system_ctx(), &Faker.fake(), make_test_command())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkPutItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkPutItemError', got '{err}'"),
            }
        }
    }

    mod update_notification {
        use super::*;

        #[tokio::test]
        async fn should_update_seen_when_success() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<NotificationRecord>();
                        faked.seen = false;
                        Ok(Some(faked))
                    })
                });
            repository
                .expect_update_notification_record()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<NotificationRecord>();
                        faked.seen = true;
                        Ok(Some(faked))
                    })
                });

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .update_notification(
                    &system_ctx(),
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await
                .unwrap();

            assert!(result.seen);
        }

        #[tokio::test]
        async fn should_err_notification_not_found_when_no_notification_exists() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let user_id = UserId::new();
            let origin_event_id = EventId::new();
            let actual = service
                .update_notification(
                    &system_ctx(),
                    &user_id,
                    &origin_event_id,
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await
                .unwrap_err();

            match actual {
                NotificationError::NotificationNotFound(err_user_id, err_origin_event_id) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(origin_event_id, err_origin_event_id);
                }
                err => {
                    panic!("Expected 'NotificationError::NotificationNotFound' but got '{err}'")
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let actual = service
                .update_notification(
                    &system_ctx(),
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkGetItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkGetItemError', got '{err}'"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_update_item(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::update_item::UpdateItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<NotificationRecord>();
                        faked.seen = false;
                        Ok(Some(faked))
                    })
                });
            repository
                .expect_update_notification_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let actual = service
                .update_notification(
                    &system_ctx(),
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkUpdateItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkUpdateItemError', got '{err}'"),
            }
        }
    }

    mod view_notifications {
        use super::*;

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(DynamoSdkError::construction_failure("Something went wrong"))]
        #[case::timeout(DynamoSdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(DynamoSdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(DynamoSdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(DynamoSdkError::service_error(
            aws_sdk_dynamodb::operation::query::QueryError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_query(
            #[case] expected: DynamoSdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_notification_records()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let actual = service
                .view_notifications(&Faker.fake(), &[Language::De], &Currency::Eur, &None)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkQueryError(_) => {}
                err => panic!("Expected 'NotificationError::SdkQueryError', got '{err}'"),
            }
        }
    }

    mod send_externally {
        use super::*;
        use aws_sdk_s3::{
            operation::get_object::GetObjectOutput,
            primitives::{ByteStream, SdkBody},
        };
        use common::user_search_filter_id::UserSearchFilterId;

        fn make_state_change_notification_record(
            user_id: UserId,
            origin_event_id: EventId,
        ) -> NotificationRecord {
            let mut record = make_notification_record_with_type(user_id, origin_event_id, None);
            record.external = true;
            record
        }

        fn make_already_sent_notification_record(
            user_id: UserId,
            origin_event_id: EventId,
        ) -> NotificationRecord {
            make_notification_record_with_type(
                user_id,
                origin_event_id,
                Some(NotificationTypeRecord::Email),
            )
        }

        fn mock_s3_returns_template(s3_adapter: &mut MockS3Adapter) {
            s3_adapter.expect_get_object().return_once(|_, _| {
                Box::pin(async {
                    Ok(GetObjectOutput::builder()
                        .body(ByteStream::new(SdkBody::from(
                            "<html>Hello {{title}}</html>",
                        )))
                        .build())
                })
            });
        }

        fn mock_ses_sends_email(ses_adapter: &mut MockSesAdapter) {
            ses_adapter
                .expect_send_email()
                .return_once(|_, _, _, _, _| {
                    Box::pin(async { Ok(SendEmailOutput::builder().build()) })
                });
        }

        #[tokio::test]
        async fn should_skip_sending_when_already_sent_externally() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_already_sent_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));
            // No update_notification_record expectation — must not be called.
            repository.expect_update_notification_record().never();

            let mut user_service = MockUserService::default();
            user_service.expect_find_user().never();

            let mut ses_adapter = MockSesAdapter::default();
            ses_adapter.expect_send_email().never();

            let mut s3_adapter = MockS3Adapter::default();
            s3_adapter.expect_get_object().never();

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap();

            assert!(result.notification_type.is_some());
        }

        #[tokio::test]
        async fn should_skip_sending_when_external_is_false() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let mut record = make_notification_record_with_type(user_id, origin_event_id, None);
            record.external = false;
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));
            // Must not update, call SES or S3 when external=false
            repository.expect_update_notification_record().never();

            let mut user_service = MockUserService::default();
            user_service.expect_find_user().never();

            let mut ses_adapter = MockSesAdapter::default();
            ses_adapter.expect_send_email().never();

            let mut s3_adapter = MockS3Adapter::default();
            s3_adapter.expect_get_object().never();

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap();

            // Returned as-is without sending
            assert!(result.notification_type.is_none());
            assert!(!result.external);
        }

        #[tokio::test]
        async fn should_send_email_and_persist_notification_type_when_not_yet_sent() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_state_change_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            // After sending, the service persists notification_type = Email
            let mut updated_record = Faker.fake::<NotificationRecord>();
            updated_record.notification_type = Some(NotificationTypeRecord::Email);
            updated_record.user_id = user_id;
            updated_record.origin_event_id = origin_event_id;
            repository
                .expect_update_notification_record()
                .withf(move |uid, eid, update| {
                    *uid == user_id
                        && *eid == origin_event_id
                        && update.notification_type == Some(NotificationTypeRecord::Email)
                        && update.seen.is_none()
                })
                .return_once(move |_, _, _| Box::pin(async move { Ok(Some(updated_record)) }));

            let mut user_service = MockUserService::default();
            let user = make_user(user_id);
            user_service
                .expect_find_user()
                .return_once(move |_| Box::pin(async move { Ok(user) }));

            let mut ses_adapter = MockSesAdapter::default();
            mock_ses_sends_email(&mut ses_adapter);

            let mut s3_adapter = MockS3Adapter::default();
            mock_s3_returns_template(&mut s3_adapter);

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap();

            assert_eq!(result.notification_type, Some(NotificationType::Email));
        }

        #[tokio::test]
        async fn should_err_notification_not_found_when_notification_does_not_exist() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::NotificationNotFound(uid, eid) => {
                    assert_eq!(uid, user_id);
                    assert_eq!(eid, origin_event_id);
                }
                err => panic!("Expected 'NotificationNotFound', got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_err_user_not_found_when_user_does_not_exist() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_state_change_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let mut user_service = MockUserService::default();
            user_service.expect_find_user().return_once(move |_| {
                Box::pin(async move { Err(UserServiceError::UserNotFound(user_id)) })
            });

            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::UserNotFound(uid) => {
                    assert_eq!(uid, user_id);
                }
                err => panic!("Expected 'UserNotFound', got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_err_user_lookup_failed_when_user_service_returns_sdk_error() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_state_change_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let mut user_service = MockUserService::default();
            user_service.expect_find_user().return_once(move |_| {
                Box::pin(async move {
                    Err(UserServiceError::SdkGetItemError(
                        aws_sdk_dynamodb::error::SdkError::construction_failure(
                            "Simulated DynamoDB failure",
                        ),
                    ))
                })
            });

            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::UserLookupFailed(_) => {}
                err => panic!("Expected 'UserLookupFailed', got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_err_ses_send_mail_when_ses_fails() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_state_change_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let mut user_service = MockUserService::default();
            let user = make_user(user_id);
            user_service
                .expect_find_user()
                .return_once(move |_| Box::pin(async move { Ok(user) }));

            let mut ses_adapter = MockSesAdapter::default();
            ses_adapter
                .expect_send_email()
                .return_once(|_, _, _, _, _| {
                    Box::pin(async {
                        Err(aws_sdk_sesv2::error::SdkError::construction_failure(
                            "Simulated SES failure",
                        ))
                    })
                });

            let mut s3_adapter = MockS3Adapter::default();
            mock_s3_returns_template(&mut s3_adapter);

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::SdkSESSendMailError(_) => {}
                err => panic!("Expected 'SdkSESSendMailError', got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_propagate_sdk_update_error_when_persisting_notification_type_fails() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_state_change_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));
            repository
                .expect_update_notification_record()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        Err(aws_sdk_dynamodb::error::SdkError::construction_failure(
                            "Simulated UpdateItem failure",
                        ))
                    })
                });

            let mut user_service = MockUserService::default();
            let user = make_user(user_id);
            user_service
                .expect_find_user()
                .return_once(move |_| Box::pin(async move { Ok(user) }));

            let mut ses_adapter = MockSesAdapter::default();
            mock_ses_sends_email(&mut ses_adapter);

            let mut s3_adapter = MockS3Adapter::default();
            mock_s3_returns_template(&mut s3_adapter);

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::SdkUpdateItemError(_) => {}
                err => panic!("Expected 'SdkUpdateItemError', got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_use_default_language_and_currency_when_user_has_no_preferences() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_state_change_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let mut updated_record = Faker.fake::<NotificationRecord>();
            updated_record.notification_type = Some(NotificationTypeRecord::Email);
            updated_record.user_id = user_id;
            updated_record.origin_event_id = origin_event_id;
            repository
                .expect_update_notification_record()
                .return_once(move |_, _, _| Box::pin(async move { Ok(Some(updated_record)) }));

            let mut user_service = MockUserService::default();
            // User with no language/currency preferences
            let user = User {
                user_id,
                email: "test@example.com".try_into().unwrap(),
                first_name: None,
                last_name: None,
                language: None,
                currency: None,
                measurement_unit: None,
                prohibited_content_consent: false,
                tier: user::core::tier::UserTier::Free,
                role: user::core::role::UserRole::User,
                stripe_customer_id: None,
                structured_address: None,
                geo_address: None,
                partner_shops: Default::default(),
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            user_service
                .expect_find_user()
                .return_once(move |_| Box::pin(async move { Ok(user) }));

            let mut ses_adapter = MockSesAdapter::default();
            mock_ses_sends_email(&mut ses_adapter);

            let mut s3_adapter = MockS3Adapter::default();
            mock_s3_returns_template(&mut s3_adapter);

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            // Should succeed — defaults to Language::En and Currency::Eur
            let result = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap();

            assert_eq!(result.notification_type, Some(NotificationType::Email));
        }

        #[tokio::test]
        async fn should_not_call_ses_when_already_sent() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_already_sent_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let mut user_service = MockUserService::default();
            user_service.expect_find_user().never();

            let mut ses_adapter = MockSesAdapter::default();
            ses_adapter.expect_send_email().never();

            let mut s3_adapter = MockS3Adapter::default();
            s3_adapter.expect_get_object().never();

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap();

            // Notification type should be preserved from the existing record.
            assert!(result.notification_type.is_some());
        }

        #[tokio::test]
        async fn should_propagate_sdk_error_get_item_when_fetching_notification_fails() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| {
                    Box::pin(async {
                        Err(aws_sdk_dynamodb::error::SdkError::construction_failure(
                            "Simulated GetItem failure",
                        ))
                    })
                });

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::SdkGetItemError(_) => {}
                err => panic!("Expected 'SdkGetItemError', got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_err_update_item_when_update_returns_none() {
            let user_id = UserId::new();
            let origin_event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            let record = make_state_change_notification_record(user_id, origin_event_id);
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));
            repository
                .expect_update_notification_record()
                .return_once(|_, _, _| Box::pin(async { Ok(None) }));

            let mut user_service = MockUserService::default();
            let user = make_user(user_id);
            user_service
                .expect_find_user()
                .return_once(move |_| Box::pin(async move { Ok(user) }));

            let mut ses_adapter = MockSesAdapter::default();
            mock_ses_sends_email(&mut ses_adapter);

            let mut s3_adapter = MockS3Adapter::default();
            mock_s3_returns_template(&mut s3_adapter);

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::SdkUpdateItemError(_) => {}
                err => panic!("Expected 'SdkUpdateItemError', got '{err}'"),
            }
        }

        fn make_price_change_notification_record(
            user_id: UserId,
            origin_event_id: EventId,
        ) -> NotificationRecord {
            let mut record = make_notification_record_with_type(user_id, origin_event_id, None);
            record.external = true;
            // WatchlistPriceChanged → PriceChange payload → WatchlistUpdatePrice template
            record.notification_reason =
                crate::dynamodb::notification_reason_record::NotificationReasonRecord::WatchlistPriceChanged;
            record
        }

        fn make_watchlist_state_change_notification_record(
            user_id: UserId,
            origin_event_id: EventId,
        ) -> NotificationRecord {
            let mut record = make_notification_record_with_type(user_id, origin_event_id, None);
            record.external = true;
            // WatchlistStateChanged → StateChange payload → WatchlistUpdateState template
            record.notification_reason =
                crate::dynamodb::notification_reason_record::NotificationReasonRecord::WatchlistStateChanged;
            record
        }

        fn make_search_filter_match_notification_record(
            user_id: UserId,
            origin_event_id: EventId,
        ) -> NotificationRecord {
            let mut record = make_notification_record_with_type(user_id, origin_event_id, None);
            record.external = true;
            record.notification_reason =
                crate::dynamodb::notification_reason_record::NotificationReasonRecord::SearchFilterMatch;
            record.user_search_filter_id = Some(UserSearchFilterId::new());
            record.user_search_filter_name = Some("My Filter".into());
            record
        }

        #[rstest::rstest]
        #[case::price_change(
            {
                let (uid, eid) = (UserId::new(), EventId::new());
                (make_price_change_notification_record(uid, eid), uid, eid)
            },
            "WATCHLIST_UPDATE_PRICE"
        )]
        #[case::state_change(
            {
                let (uid, eid) = (UserId::new(), EventId::new());
                (make_watchlist_state_change_notification_record(uid, eid), uid, eid)
            },
            "WATCHLIST_UPDATE_STATE"
        )]
        #[case::search_filter_match(
            {
                let (uid, eid) = (UserId::new(), EventId::new());
                (make_search_filter_match_notification_record(uid, eid), uid, eid)
            },
            "SEARCH_FILTER_MATCH"
        )]
        #[tokio::test]
        async fn should_send_email_with_correct_message_tag_for_template_type(
            #[case] record_and_ids: (NotificationRecord, UserId, EventId),
            #[case] expected_tag_value: &'static str,
        ) {
            let (record, user_id, origin_event_id) = record_and_ids;

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));

            let mut updated_record = Faker.fake::<NotificationRecord>();
            updated_record.notification_type =
                Some(crate::dynamodb::notification_type_record::NotificationTypeRecord::Email);
            updated_record.user_id = user_id;
            updated_record.origin_event_id = origin_event_id;
            repository
                .expect_update_notification_record()
                .return_once(move |_, _, _| Box::pin(async move { Ok(Some(updated_record)) }));

            let mut user_service = MockUserService::default();
            let user = make_user(user_id);
            user_service
                .expect_find_user()
                .return_once(move |_| Box::pin(async move { Ok(user) }));

            let mut ses_adapter = MockSesAdapter::default();
            ses_adapter
                .expect_send_email()
                .withf(move |_, _, _, _, tags| {
                    tags.len() == 1
                        && tags[0].name == "template_type"
                        && tags[0].value == expected_tag_value
                })
                .return_once(|_, _, _, _, _| {
                    Box::pin(async { Ok(SendEmailOutput::builder().build()) })
                });

            let mut s3_adapter = MockS3Adapter::default();
            mock_s3_returns_template(&mut s3_adapter);

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            service
                .send_externally(&system_ctx(), &user_id, &origin_event_id)
                .await
                .unwrap();
        }
    }

    mod derive_mail_template_tests {
        use super::*;
        use common::language::data::LanguageData;

        #[test]
        fn should_derive_price_change_template_for_price_change_payload() {
            let payload = NotificationPayload::Watchlist {
                product_id: Faker.fake(),
                shop_id: Faker.fake(),
                shops_product_id: "test".into(),
                shop_slug_id: Faker.fake(),
                product_slug_id: Faker.fake(),
                shop_name: "Shop".into(),
                title: HashMap::from([(Language::En, "Title".into())]),
                image: None,
                url: url::Url::parse("https://example.com/item/1").unwrap(),
                view_url: url::Url::parse(
                    "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                )
                .unwrap(),
                watchlist_payload: NotificationWatchlistPayload::PriceChange {
                    old_price: HashMap::new(),
                    new_price: HashMap::new(),
                },
            };

            let template = derive_mail_template(&payload, &Language::De);
            assert_eq!(
                template.template_type,
                MailTemplateType::WatchlistUpdatePrice
            );
            assert_eq!(template.language, LanguageData::De);
        }

        #[test]
        fn should_derive_state_change_template_for_state_change_payload() {
            let payload = NotificationPayload::Watchlist {
                product_id: Faker.fake(),
                shop_id: Faker.fake(),
                shops_product_id: "test".into(),
                shop_slug_id: Faker.fake(),
                product_slug_id: Faker.fake(),
                shop_name: "Shop".into(),
                title: HashMap::from([(Language::En, "Title".into())]),
                image: None,
                url: url::Url::parse("https://example.com/item/1").unwrap(),
                view_url: url::Url::parse(
                    "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                )
                .unwrap(),
                watchlist_payload: NotificationWatchlistPayload::StateChange {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Sold,
                },
            };

            let template = derive_mail_template(&payload, &Language::Fr);
            assert_eq!(
                template.template_type,
                MailTemplateType::WatchlistUpdateState
            );
            assert_eq!(template.language, LanguageData::Fr);
        }

        #[rstest::rstest]
        #[case::de(Language::De, LanguageData::De)]
        #[case::en(Language::En, LanguageData::En)]
        #[case::fr(Language::Fr, LanguageData::Fr)]
        #[case::es(Language::Es, LanguageData::Es)]
        #[case::it(Language::It, LanguageData::It)]
        fn should_map_language_to_language_data_for_template(
            #[case] language: Language,
            #[case] expected_data: LanguageData,
        ) {
            let payload = NotificationPayload::Watchlist {
                product_id: Faker.fake(),
                shop_id: Faker.fake(),
                shops_product_id: "test".into(),
                shop_slug_id: Faker.fake(),
                product_slug_id: Faker.fake(),
                shop_name: "Shop".into(),
                title: HashMap::from([(Language::En, "Title".into())]),
                image: None,
                url: url::Url::parse("https://example.com/item/1").unwrap(),
                view_url: url::Url::parse(
                    "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                )
                .unwrap(),
                watchlist_payload: NotificationWatchlistPayload::StateChange {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Sold,
                },
            };

            let template = derive_mail_template(&payload, &language);
            assert_eq!(template.language, expected_data);
        }
    }

    mod build_email_subject_tests {
        use super::*;

        fn make_watchlist_payload_state(
            title: HashMap<Language, product::core::title::Title>,
            old_state: ProductState,
            new_state: ProductState,
        ) -> NotificationPayload {
            NotificationPayload::Watchlist {
                product_id: Faker.fake(),
                shop_id: Faker.fake(),
                shops_product_id: "test".into(),
                shop_slug_id: Faker.fake(),
                product_slug_id: Faker.fake(),
                shop_name: "Shop".into(),
                title,
                image: None,
                url: url::Url::parse("https://example.com/item/1").unwrap(),
                view_url: url::Url::parse(
                    "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                )
                .unwrap(),
                watchlist_payload: NotificationWatchlistPayload::StateChange {
                    old_state,
                    new_state,
                },
            }
        }

        fn make_watchlist_payload_price(
            title: HashMap<Language, product::core::title::Title>,
        ) -> NotificationPayload {
            NotificationPayload::Watchlist {
                product_id: Faker.fake(),
                shop_id: Faker.fake(),
                shops_product_id: "test".into(),
                shop_slug_id: Faker.fake(),
                product_slug_id: Faker.fake(),
                shop_name: "Shop".into(),
                title,
                image: None,
                url: url::Url::parse("https://example.com/item/1").unwrap(),
                view_url: url::Url::parse(
                    "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                )
                .unwrap(),
                watchlist_payload: NotificationWatchlistPayload::PriceChange {
                    old_price: HashMap::new(),
                    new_price: HashMap::new(),
                },
            }
        }

        #[test]
        fn should_build_english_price_change_subject() {
            let title = HashMap::from([(Language::En, "Antique Vase".into())]);
            let payload = make_watchlist_payload_price(title);
            let subject = build_email_subject(&payload, &Language::En);
            assert_eq!(subject, "Price change: Antique Vase");
        }

        #[test]
        fn should_build_german_price_change_subject() {
            let title = HashMap::from([(Language::De, "Antike Vase".into())]);
            let payload = make_watchlist_payload_price(title);
            let subject = build_email_subject(&payload, &Language::De);
            assert_eq!(subject, "Preisänderung: Antike Vase");
        }

        #[test]
        fn should_build_french_price_change_subject() {
            let title = HashMap::from([(Language::Fr, "Vase antique".into())]);
            let payload = make_watchlist_payload_price(title);
            let subject = build_email_subject(&payload, &Language::Fr);
            assert_eq!(subject, "Changement de prix : Vase antique");
        }

        #[test]
        fn should_build_spanish_price_change_subject() {
            let title = HashMap::from([(Language::Es, "Jarrón antiguo".into())]);
            let payload = make_watchlist_payload_price(title);
            let subject = build_email_subject(&payload, &Language::Es);
            assert_eq!(subject, "Cambio de precio: Jarrón antiguo");
        }

        #[test]
        fn should_build_italian_price_change_subject() {
            let title = HashMap::from([(Language::It, "Vaso antico".into())]);
            let payload = make_watchlist_payload_price(title);
            let subject = build_email_subject(&payload, &Language::It);
            assert_eq!(subject, "Variazione di prezzo: Vaso antico");
        }

        #[test]
        fn should_build_english_state_change_subject_with_sold_state() {
            let title = HashMap::from([(Language::En, "Antique Vase".into())]);
            let payload =
                make_watchlist_payload_state(title, ProductState::Listed, ProductState::Sold);
            let subject = build_email_subject(&payload, &Language::En);
            assert_eq!(subject, "Status change (Sold): Antique Vase");
        }

        #[test]
        fn should_build_german_state_change_subject_with_sold_state() {
            let title = HashMap::from([(Language::De, "Antike Vase".into())]);
            let payload =
                make_watchlist_payload_state(title, ProductState::Listed, ProductState::Sold);
            let subject = build_email_subject(&payload, &Language::De);
            assert_eq!(subject, "Statusänderung (Verkauft): Antike Vase");
        }

        #[test]
        fn should_fallback_to_unknown_when_title_not_available_for_language() {
            let title = HashMap::new();
            let payload = make_watchlist_payload_price(title);
            let subject = build_email_subject(&payload, &Language::En);
            assert_eq!(subject, "Price change: Unknown");
        }

        #[test]
        fn should_resolve_title_for_english_when_requested_language_unavailable() {
            let title = HashMap::from([(Language::En, "English Title".into())]);
            let payload = make_watchlist_payload_price(title);
            // Requesting French but only English is available
            let subject = build_email_subject(&payload, &Language::Fr);
            // Language::resolve falls back to English
            assert_eq!(subject, "Changement de prix : English Title");
        }
    }

    mod build_email_template_data_tests {
        use super::*;
        use crate::core::notification::NotificationSearchFilterPayload;
        use common::partner_shop_application_id::PartnerShopApplicationId;

        #[test]
        fn should_include_product_fields_for_state_change() {
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test-product-123".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Test Shop".into(),
                    title: HashMap::from([(Language::En, "Antique Vase".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            assert_eq!(data["title"], "Antique Vase");
            assert_eq!(data["shop_name"], "Test Shop");
            assert_eq!(data["old_state"], "Listed");
            assert_eq!(data["new_state"], "Sold");
            assert_eq!(data["notification_type"], "state_change");
        }

        #[test]
        fn should_include_watchlist_image_url_when_present() {
            let image_url =
                url::Url::parse("https://example.com/item/1.png?size=large&fit=cover").unwrap();
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test-product-123".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Test Shop".into(),
                    title: HashMap::from([(Language::En, "Antique Vase".into())]),
                    image: Some(ProductImage {
                        url: image_url.clone(),
                        prohibited_content: ProhibitedContent::None,
                    }),
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            assert_eq!(data["image_url"], image_url.as_str());
        }

        #[test]
        fn should_include_price_fields_for_price_change() {
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test-product-123".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Test Shop".into(),
                    title: HashMap::from([(Language::En, "Antique Vase".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::PriceChange {
                        old_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(10000u64))]),
                        new_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(8000u64))]),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            assert_eq!(data["notification_type"], "price_change");
            // old_price and new_price should be present as human-readable strings
            assert!(data["old_price"].is_string());
            assert!(data["new_price"].is_string());
        }

        #[test]
        fn should_not_include_old_price_when_none_available() {
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Shop".into(),
                    title: HashMap::from([(Language::En, "Title".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::PriceChange {
                        old_price: HashMap::new(),
                        new_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(5000u64))]),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            assert!(data.get("old_price").is_none());
            assert!(data["new_price"].is_string());
        }

        #[test]
        fn should_use_localized_state_names_for_german() {
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Shop".into(),
                    title: HashMap::from([(Language::De, "Antike Vase".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::De, &Currency::Eur, None);

            assert_eq!(data["old_state"], "Gelistet");
            assert_eq!(data["new_state"], "Verkauft");
            assert_eq!(data["title"], "Antike Vase");
        }

        #[test]
        fn should_include_user_first_name_when_provided() {
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Shop".into(),
                    title: HashMap::from([(Language::En, "Title".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let first_name = user::core::first_name::FirstName::from("Thomas");
            let data = build_email_template_data(
                &notification,
                &Language::En,
                &Currency::Eur,
                Some(&first_name),
            );

            assert_eq!(data["user_first_name"], "Thomas");
        }

        #[test]
        fn should_not_include_user_first_name_when_none() {
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Shop".into(),
                    title: HashMap::from([(Language::En, "Title".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            assert!(data.get("user_first_name").is_none());
        }

        #[test]
        fn should_include_search_filter_image_url_when_present() {
            let image_url =
                url::Url::parse("https://example.com/item/1.png?size=large&fit=cover").unwrap();
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::SearchFilter {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test-product-123".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Test Shop".into(),
                    title: HashMap::from([(Language::En, "Antique Vase".into())]),
                    image: Some(ProductImage {
                        url: image_url.clone(),
                        prohibited_content: ProhibitedContent::None,
                    }),
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    search_filter_payload: NotificationSearchFilterPayload {
                        user_search_filter_id:
                            common::user_search_filter_id::UserSearchFilterId::new(),
                        user_search_filter_name: "Victorian Furniture".into(),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            assert_eq!(data["image_url"], image_url.as_str());
        }

        #[test]
        fn should_include_partner_application_image_url_when_present() {
            let image_url = url::Url::parse("https://example.com/logo.png").unwrap();
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::PartnerApplication {
                    shop_name: "Test Shop".into(),
                    image: Some(image_url.clone()),
                    partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                        partner_application_id: PartnerShopApplicationId::new(),
                    },
                },
                seen: false,
                external: true,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            assert_eq!(data["image_url"], image_url.as_str());
        }
    }

    mod resolve_template_tests {
        use super::*;
        use aws_sdk_s3::{
            operation::get_object::GetObjectOutput,
            primitives::{ByteStream, SdkBody},
        };
        use common::language::data::LanguageData;

        #[tokio::test]
        async fn should_reuse_template_when_in_cache() {
            let repository = MockNotificationDynamoDbRepository::default();
            let user_service = MockUserService::default();
            let mut ses_adapter = MockSesAdapter::default();
            ses_adapter.expect_send_email().never();
            let mut s3_adapter = MockS3Adapter::default();
            s3_adapter.expect_get_object().never();

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let template = MailTemplate {
                template_type: MailTemplateType::WatchlistUpdatePrice,
                language: LanguageData::De,
            };

            // Seed the cache
            let cache = TEMPLATE_CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())));
            cache
                .write()
                .await
                .insert(template, "cached-html".to_owned());

            let actual = service.resolve_template(template).await.unwrap();
            assert_eq!("cached-html", actual);
        }

        #[tokio::test]
        async fn should_fetch_from_s3_and_cache_when_not_in_cache() {
            let repository = MockNotificationDynamoDbRepository::default();
            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let mut s3_adapter = MockS3Adapter::default();
            s3_adapter
                .expect_get_object()
                .withf(|bucket, key| {
                    bucket == "test-bucket"
                        && key == "test-stage/test-sha/mjml/watchlist/product-update/state/en.html"
                })
                .return_once(|_, _| {
                    Box::pin(async {
                        Ok(GetObjectOutput::builder()
                            .body(ByteStream::new(SdkBody::from(
                                "<html>State Template</html>",
                            )))
                            .build())
                    })
                });

            let template = MailTemplate {
                template_type: MailTemplateType::WatchlistUpdateState,
                language: LanguageData::En,
            };

            // Clear cache
            let cache = TEMPLATE_CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())));
            cache.write().await.remove(&template);

            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let actual = service.resolve_template(template).await.unwrap();

            assert_eq!("<html>State Template</html>", actual);

            // Verify it's now cached
            let cached = cache.read().await.get(&template).cloned();
            assert_eq!(cached, Some("<html>State Template</html>".to_owned()));
        }
    }

    mod update_notifications {
        use super::*;

        #[tokio::test]
        async fn should_update_all_records() {
            let user_id = UserId::new();
            let mut record1: NotificationRecord = Faker.fake();
            record1.user_id = user_id;
            let mut record2: NotificationRecord = Faker.fake();
            record2.user_id = user_id;

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_all_notification_records()
                .return_once(move |_| {
                    Box::pin(async move { Ok(vec![record1.clone(), record2.clone()]) })
                });
            repository
                .expect_update_notification_record()
                .times(2)
                .returning(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_query_notification_records()
                .return_once(|_, _, _| Box::pin(async { Ok(vec![]) }));
            repository
                .expect_count_notification_records()
                .return_once(|_, _, _| Box::pin(async { Ok(0) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .update_notifications(
                    &system_ctx(),
                    &user_id,
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await;

            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_return_first_page_of_notifications() {
            let user_id = UserId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_all_notification_records()
                .return_once(|_| Box::pin(async { Ok(vec![]) }));
            repository
                .expect_query_notification_records()
                .return_once(|_, _, _| Box::pin(async { Ok(vec![]) }));
            repository
                .expect_count_notification_records()
                .return_once(|_, _, _| Box::pin(async { Ok(0) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .update_notifications(
                    &system_ctx(),
                    &user_id,
                    UpdateNotificationCommand::default(),
                )
                .await;

            assert!(result.is_ok());
            let page = result.unwrap();
            assert_eq!(Some(0), page.total);
        }
    }

    mod delete_notification {
        use super::*;

        #[tokio::test]
        async fn should_return_ok_when_exists() {
            let user_id = UserId::new();
            let event_id = EventId::new();
            let mut record: NotificationRecord = Faker.fake();
            record.user_id = user_id;
            record.origin_event_id = event_id;

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(record)) }));
            repository
                .expect_delete_notification_record()
                .return_once(|_, _| {
                    Box::pin(async {
                        Ok(
                            aws_sdk_dynamodb::operation::delete_item::DeleteItemOutput::builder()
                                .build(),
                        )
                    })
                });

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .delete_notification(&system_ctx(), &user_id, &event_id)
                .await;

            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_return_not_found_when_not_exists() {
            let user_id = UserId::new();
            let event_id = EventId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            repository.expect_delete_notification_record().never();

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service
                .delete_notification(&system_ctx(), &user_id, &event_id)
                .await;

            assert!(result.is_err());
            match result.unwrap_err() {
                NotificationError::NotificationNotFound(_, _) => {}
                err => panic!("Expected NotificationNotFound, got '{err}'"),
            }
        }
    }

    mod delete_all_notifications {
        use super::*;

        #[tokio::test]
        async fn should_delete_all_records() {
            let user_id = UserId::new();
            let mut record1: NotificationRecord = Faker.fake();
            record1.user_id = user_id;
            let mut record2: NotificationRecord = Faker.fake();
            record2.user_id = user_id;

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_all_notification_records()
                .return_once(move |_| Box::pin(async move { Ok(vec![record1, record2]) }));
            repository
                .expect_delete_notification_records()
                .return_once(|_, _| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                            .build())
                    })
                });

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service.delete_notifications(&system_ctx(), &user_id).await;

            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_succeed_when_no_notifications() {
            let user_id = UserId::new();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_all_notification_records()
                .return_once(|_| Box::pin(async { Ok(vec![]) }));
            repository.expect_delete_notification_records().never();

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);
            let result = service.delete_notifications(&system_ctx(), &user_id).await;

            assert!(result.is_ok());
        }
    }

    mod find_notifications_by_product {
        use super::*;
        use common::product_id::ProductId;

        #[tokio::test]
        async fn should_return_empty_when_no_notifications_for_product() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_product_notification_records()
                .return_once(|_, _, _, _| Box::pin(async { Ok(vec![]) }));

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .find_notifications_by_product(&UserId::new(), &ProductId::new(), None, false)
                .await
                .unwrap();

            assert!(actual.is_empty());
        }

        #[tokio::test]
        async fn should_return_notifications_when_records_exist() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_product_notification_records()
                .return_once(|_, _, _, _| {
                    Box::pin(async {
                        let record: NotificationRecord = Faker.fake();
                        Ok(vec![record])
                    })
                });

            let user_service = MockUserService::default();
            let ses_adapter = MockSesAdapter::default();
            let s3_adapter = MockS3Adapter::default();
            let service = make_service(&repository, &user_service, &ses_adapter, &s3_adapter);

            let actual = service
                .find_notifications_by_product(&UserId::new(), &ProductId::new(), Some(10), false)
                .await
                .unwrap();

            assert_eq!(actual.len(), 1);
        }
    }

    mod template_rendering_tests {
        use super::*;
        use crate::core::notification::NotificationSearchFilterPayload;
        use common::partner_shop_application_id::PartnerShopApplicationId;
        use common::user_search_filter_id::UserSearchFilterId;
        use rstest::rstest;

        const ALL_TEMPLATE_PATHS: &[&str] = &[
            "mjml/watchlist/product-update/price/de.mjml",
            "mjml/watchlist/product-update/price/en.mjml",
            "mjml/watchlist/product-update/price/es.mjml",
            "mjml/watchlist/product-update/price/fr.mjml",
            "mjml/watchlist/product-update/price/it.mjml",
            "mjml/watchlist/product-update/state/de.mjml",
            "mjml/watchlist/product-update/state/en.mjml",
            "mjml/watchlist/product-update/state/es.mjml",
            "mjml/watchlist/product-update/state/fr.mjml",
            "mjml/watchlist/product-update/state/it.mjml",
            "mjml/search-filter/match/de.mjml",
            "mjml/search-filter/match/en.mjml",
            "mjml/search-filter/match/es.mjml",
            "mjml/search-filter/match/fr.mjml",
            "mjml/search-filter/match/it.mjml",
            "mjml/partner-application/approval/de.mjml",
            "mjml/partner-application/approval/en.mjml",
            "mjml/partner-application/approval/es.mjml",
            "mjml/partner-application/approval/fr.mjml",
            "mjml/partner-application/approval/it.mjml",
            "mjml/partner-application/rejection/de.mjml",
            "mjml/partner-application/rejection/en.mjml",
            "mjml/partner-application/rejection/es.mjml",
            "mjml/partner-application/rejection/fr.mjml",
            "mjml/partner-application/rejection/it.mjml",
        ];

        fn required_imprint_fields(lang: &str) -> &'static [&'static str] {
            match lang {
                "en" => &[
                    "Imprint",
                    "Trade name: Aura Historia",
                    "Owner: Julian Bruder Einzelunternehmen",
                    "Address: Hardenbergstraße 80, 04275 Leipzig, Germany",
                    "Contact: <a href=\"mailto:contact@aura-historia.com\"",
                    "VAT ID: requested",
                ],
                "de" => &[
                    "Impressum",
                    "Handelsname: Aura Historia",
                    "Inhaber: Julian Bruder Einzelunternehmen",
                    "Anschrift: Hardenbergstraße 80, 04275 Leipzig, Germany",
                    "Kontakt: <a href=\"mailto:contact@aura-historia.com\"",
                    "USt-IdNr.: angefragt",
                ],
                "fr" => &[
                    "Mentions légales",
                    "Nom commercial : Aura Historia",
                    "Propriétaire : Julian Bruder Einzelunternehmen",
                    "Adresse : Hardenbergstraße 80, 04275 Leipzig, Germany",
                    "Contact : <a href=\"mailto:contact@aura-historia.com\"",
                    "N° de TVA : demandée",
                ],
                "es" => &[
                    "Aviso legal",
                    "Nombre comercial: Aura Historia",
                    "Titular: Julian Bruder Einzelunternehmen",
                    "Dirección: Hardenbergstraße 80, 04275 Leipzig, Germany",
                    "Contacto: <a href=\"mailto:contact@aura-historia.com\"",
                    "N.º de IVA: solicitado",
                ],
                "it" => &[
                    "Note legali",
                    "Nome commerciale: Aura Historia",
                    "Titolare: Julian Bruder Einzelunternehmen",
                    "Indirizzo: Hardenbergstraße 80, 04275 Leipzig, Germany",
                    "Contatto: <a href=\"mailto:contact@aura-historia.com\"",
                    "Partita IVA: richiesta",
                ],
                _ => panic!("Unknown language code: {lang}"),
            }
        }

        fn load_template(relative_path: &str) -> String {
            let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap();
            let path = workspace_root.join(relative_path);
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("Failed to load template {}: {e}", path.display()))
        }

        fn assert_no_unreplaced_handlebars(rendered: &str, template_path: &str) {
            let mut pos = 0;
            let mut unreplaced = Vec::new();
            while let Some(start) = rendered[pos..].find("{{") {
                let abs_start = pos + start;
                let rest = &rendered[abs_start + 2..];
                // Skip handlebars block helpers: {{#...}}, {{/...}}, {{!--...}}, {{else}}, {{>...}}
                if rest.starts_with('#')
                    || rest.starts_with('/')
                    || rest.starts_with("!--")
                    || rest.starts_with("else")
                    || rest.starts_with('>')
                {
                    pos = abs_start + 2;
                    continue;
                }
                let snippet_end = (abs_start + 40).min(rendered.len());
                unreplaced.push(rendered[abs_start..snippet_end].to_string());
                pos = abs_start + 2;
            }
            assert!(
                unreplaced.is_empty(),
                "Template '{template_path}' has unreplaced handlebars: {unreplaced:?}"
            );
        }

        fn make_watchlist_price_notification() -> Notification {
            Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "antique-vase-001".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Heritage Antiques".into(),
                    title: HashMap::from([
                        (Language::En, "Victorian Writing Desk".into()),
                        (Language::De, "Viktorianischer Schreibtisch".into()),
                        (Language::Fr, "Bureau d'écriture victorien".into()),
                        (Language::Es, "Escritorio victoriano".into()),
                        (Language::It, "Scrivania vittoriana".into()),
                    ]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://prf.hn/click/camref:abc/destination:https%3A%2F%2Fexample.com%2Fitem%2F1",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::PriceChange {
                        old_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(10000u64))]),
                        new_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(8500u64))]),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        fn make_watchlist_state_notification() -> Notification {
            Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "antique-vase-001".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Heritage Antiques".into(),
                    title: HashMap::from([
                        (Language::En, "Victorian Writing Desk".into()),
                        (Language::De, "Viktorianischer Schreibtisch".into()),
                        (Language::Fr, "Bureau d'écriture victorien".into()),
                        (Language::Es, "Escritorio victoriano".into()),
                        (Language::It, "Scrivania vittoriana".into()),
                    ]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://prf.hn/click/camref:abc/destination:https%3A%2F%2Fexample.com%2Fitem%2F1",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        fn make_search_filter_notification() -> Notification {
            Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::SearchFilter {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "antique-vase-001".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Heritage Antiques".into(),
                    title: HashMap::from([
                        (Language::En, "Victorian Writing Desk".into()),
                        (Language::De, "Viktorianischer Schreibtisch".into()),
                        (Language::Fr, "Bureau d'écriture victorien".into()),
                        (Language::Es, "Escritorio victoriano".into()),
                        (Language::It, "Scrivania vittoriana".into()),
                    ]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    search_filter_payload: NotificationSearchFilterPayload {
                        user_search_filter_id: UserSearchFilterId::new(),
                        user_search_filter_name: "Victorian Furniture".into(),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        fn language_for_code(code: &str) -> Language {
            match code {
                "en" => Language::En,
                "de" => Language::De,
                "fr" => Language::Fr,
                "es" => Language::Es,
                "it" => Language::It,
                _ => panic!("Unknown language code: {code}"),
            }
        }

        fn make_product_image(url: &str) -> ProductImage {
            ProductImage {
                url: url::Url::parse(url).unwrap(),
                prohibited_content: ProhibitedContent::None,
            }
        }

        fn assert_two_button_template_links(
            rendered: &str,
            data: &serde_json::Value,
            template_path: &str,
        ) {
            let shop_slug_id = data["shop_slug_id"].as_str().unwrap();
            let product_slug_id = data["product_slug_id"].as_str().unwrap();
            let aura_historia_product_url = format!(
                "https://aura-historia.com/shops/{shop_slug_id}/products/{product_slug_id}"
            );
            let merchant_view_url = data["view_url"].as_str().unwrap();
            let merchant_view_url_escaped = handlebars::html_escape(merchant_view_url);

            assert!(
                rendered.contains(&format!("href=\"{aura_historia_product_url}\"")),
                "Template '{template_path}' should contain Aura Historia product CTA: {aura_historia_product_url}"
            );
            assert!(
                rendered.contains(&format!("href=\"{merchant_view_url_escaped}\"")),
                "Template '{template_path}' should contain merchant CTA: {merchant_view_url}"
            );
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_watchlist_price_template_without_unreplaced_handlebars_for(
            #[case] lang: &str,
        ) {
            let template_path = format!("mjml/watchlist/product-update/price/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_watchlist_price_notification();
            let language = language_for_code(lang);
            let first_name = user::core::first_name::FirstName::from("Thomas");
            let data = build_email_template_data(
                &notification,
                &language,
                &Currency::Eur,
                Some(&first_name),
            );

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_watchlist_state_template_without_unreplaced_handlebars_for(
            #[case] lang: &str,
        ) {
            let template_path = format!("mjml/watchlist/product-update/state/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_watchlist_state_notification();
            let language = language_for_code(lang);
            let first_name = user::core::first_name::FirstName::from("Thomas");
            let data = build_email_template_data(
                &notification,
                &language,
                &Currency::Eur,
                Some(&first_name),
            );

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_search_filter_template_without_unreplaced_handlebars_for(
            #[case] lang: &str,
        ) {
            let template_path = format!("mjml/search-filter/match/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_search_filter_notification();
            let language = language_for_code(lang);
            let first_name = user::core::first_name::FirstName::from("Thomas");
            let data = build_email_template_data(
                &notification,
                &language,
                &Currency::Eur,
                Some(&first_name),
            );

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_watchlist_price_template_without_user_first_name_for(#[case] lang: &str) {
            let template_path = format!("mjml/watchlist/product-update/price/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_watchlist_price_notification();
            let language = language_for_code(lang);
            let data = build_email_template_data(&notification, &language, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
            assert!(
                !rendered.contains("Thomas"),
                "Template should not contain user first name when not provided"
            );
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_watchlist_price_template_with_missing_old_price_for(#[case] lang: &str) {
            let template_path = format!("mjml/watchlist/product-update/price/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test".into(),
                    shop_slug_id: common::shop_slug_id::ShopSlugId::from("test-shop"),
                    product_slug_id: common::product_slug_id::ProductSlugId::from("test-product"),
                    shop_name: "Shop".into(),
                    title: HashMap::from([(language_for_code(lang), "Title".into())]),
                    image: None,
                    url: url::Url::parse("https://example.com/item/1").unwrap(),
                    view_url: url::Url::parse(
                        "https://example.com/item/1?utm_source=aura_historia&utm_medium=referral",
                    )
                    .unwrap(),
                    watchlist_payload: NotificationWatchlistPayload::PriceChange {
                        old_price: HashMap::new(),
                        new_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(5000u64))]),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let language = language_for_code(lang);
            let data = build_email_template_data(&notification, &language, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
        }

        #[test]
        fn should_route_watchlist_price_ctas_to_aura_historia_and_merchant() {
            let template_path = "mjml/watchlist/product-update/price/en.mjml";
            let template = load_template(template_path);
            let notification = make_watchlist_price_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert_two_button_template_links(&rendered, &data, template_path);
        }

        #[test]
        fn should_route_watchlist_state_ctas_to_aura_historia_and_merchant() {
            let template_path = "mjml/watchlist/product-update/state/en.mjml";
            let template = load_template(template_path);
            let notification = make_watchlist_state_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert_two_button_template_links(&rendered, &data, template_path);
        }

        #[test]
        fn should_route_search_filter_ctas_to_aura_historia_and_merchant() {
            let template_path = "mjml/search-filter/match/en.mjml";
            let template = load_template(template_path);
            let notification = make_search_filter_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert_two_button_template_links(&rendered, &data, template_path);
        }

        #[test]
        fn should_include_search_filter_link_in_rendered_search_filter_template() {
            let template_path = "mjml/search-filter/match/en.mjml";
            let template = load_template(template_path);
            let notification = make_search_filter_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            let filter_id = data["search_filter_id"].as_str().unwrap();
            let expected_url = format!("https://aura-historia.com/search-filters/{filter_id}");
            assert!(
                rendered.contains(&expected_url),
                "Rendered template should contain search filter URL: {expected_url}"
            );
        }

        #[test]
        fn should_include_watchlist_link_in_rendered_watchlist_template() {
            let template_path = "mjml/watchlist/product-update/price/en.mjml";
            let template = load_template(template_path);
            let notification = make_watchlist_price_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains("https://aura-historia.com/watchlist"),
                "Rendered template should contain watchlist URL"
            );
        }

        #[test]
        fn should_include_contact_email_and_complete_imprint_in_all_templates() {
            for template_path in ALL_TEMPLATE_PATHS {
                let template = load_template(template_path);
                let lang = template_path
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .strip_suffix(".mjml")
                    .unwrap();
                for required_field in required_imprint_fields(lang) {
                    assert!(
                        template.contains(required_field),
                        "Template '{template_path}' should contain imprint field '{required_field}'"
                    );
                }
                assert!(
                    !template.contains("julian.bruder@aura-historia.com"),
                    "Template '{template_path}' should NOT contain the personal email address"
                );
                assert!(
                    !template.contains("Personal email:"),
                    "Template '{template_path}' should NOT contain the personal email label"
                );
                assert!(
                    !template.contains("support@aura-historia.com"),
                    "Template '{template_path}' should NOT use support@aura-historia.com"
                );
            }
        }

        #[test]
        fn should_include_user_first_name_in_rendered_template_when_provided() {
            let template_path = "mjml/watchlist/product-update/price/en.mjml";
            let template = load_template(template_path);
            let notification = make_watchlist_price_notification();
            let first_name = user::core::first_name::FirstName::from("Thomas");
            let data = build_email_template_data(
                &notification,
                &Language::En,
                &Currency::Eur,
                Some(&first_name),
            );

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains("Hello Thomas,"),
                "Rendered template should contain personalized greeting"
            );
        }

        #[test]
        fn should_include_shop_name_in_rendered_template() {
            let template_path = "mjml/watchlist/product-update/price/en.mjml";
            let template = load_template(template_path);
            let notification = make_watchlist_price_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains("Heritage Antiques"),
                "Rendered template should contain shop name"
            );
        }

        #[test]
        fn should_include_search_filter_name_in_rendered_template() {
            let template_path = "mjml/search-filter/match/en.mjml";
            let template = load_template(template_path);
            let notification = make_search_filter_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains("Victorian Furniture"),
                "Rendered template should contain search filter name"
            );
        }

        #[test]
        fn should_include_product_image_in_rendered_watchlist_price_template() {
            let template_path = "mjml/watchlist/product-update/price/en.mjml";
            let template = load_template(template_path);
            let mut notification = make_watchlist_price_notification();
            let image_url =
                "https://example.com/image.png?size=large&fit=cover&title=victorian%20desk";
            if let NotificationPayload::Watchlist { image, .. } =
                &mut notification.notification_payload
            {
                *image = Some(make_product_image(image_url));
            }
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains(&format!("src=\"{}\"", handlebars::html_escape(image_url))),
                "Rendered template should contain product image"
            );
        }

        #[test]
        fn should_include_product_image_in_rendered_watchlist_state_template() {
            let template_path = "mjml/watchlist/product-update/state/en.mjml";
            let template = load_template(template_path);
            let mut notification = make_watchlist_state_notification();
            let image_url =
                "https://example.com/image.png?size=large&fit=cover&title=victorian%20desk";
            if let NotificationPayload::Watchlist { image, .. } =
                &mut notification.notification_payload
            {
                *image = Some(make_product_image(image_url));
            }
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains(&format!("src=\"{}\"", handlebars::html_escape(image_url))),
                "Rendered template should contain product image"
            );
        }

        #[test]
        fn should_include_product_image_in_rendered_search_filter_template() {
            let template_path = "mjml/search-filter/match/en.mjml";
            let template = load_template(template_path);
            let mut notification = make_search_filter_notification();
            let image_url =
                "https://example.com/image.png?size=large&fit=cover&title=victorian%20desk";
            if let NotificationPayload::SearchFilter { image, .. } =
                &mut notification.notification_payload
            {
                *image = Some(make_product_image(image_url));
            }
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains(&format!("src=\"{}\"", handlebars::html_escape(image_url))),
                "Rendered template should contain product image"
            );
        }

        fn make_partner_application_approval_notification() -> Notification {
            Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::PartnerApplication {
                    shop_name: "Heritage Antiques".into(),
                    image: None,
                    partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                        partner_application_id: PartnerShopApplicationId::new(),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        fn make_partner_application_rejection_notification() -> Notification {
            Notification {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_id: NotificationId::new(),
                notification_type: None,
                notification_payload: NotificationPayload::PartnerApplication {
                    shop_name: "Heritage Antiques".into(),
                    image: None,
                    partner_application_payload: NotificationPartnerApplicationPayload::Rejected {
                        partner_application_id: PartnerShopApplicationId::new(),
                    },
                },
                seen: false,
                external: false,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_partner_application_approval_template_without_unreplaced_handlebars_for(
            #[case] lang: &str,
        ) {
            let template_path = format!("mjml/partner-application/approval/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_partner_application_approval_notification();
            let language = language_for_code(lang);
            let first_name = user::core::first_name::FirstName::from("Thomas");
            let data = build_email_template_data(
                &notification,
                &language,
                &Currency::Eur,
                Some(&first_name),
            );

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_partner_application_rejection_template_without_unreplaced_handlebars_for(
            #[case] lang: &str,
        ) {
            let template_path = format!("mjml/partner-application/rejection/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_partner_application_rejection_notification();
            let language = language_for_code(lang);
            let first_name = user::core::first_name::FirstName::from("Thomas");
            let data = build_email_template_data(
                &notification,
                &language,
                &Currency::Eur,
                Some(&first_name),
            );

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_partner_application_approval_template_without_user_first_name_for(
            #[case] lang: &str,
        ) {
            let template_path = format!("mjml/partner-application/approval/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_partner_application_approval_notification();
            let language = language_for_code(lang);
            let data = build_email_template_data(&notification, &language, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
            assert!(
                !rendered.contains("Thomas"),
                "Template should not contain user first name when not provided"
            );
        }

        #[rstest]
        #[case("en")]
        #[case("de")]
        #[case("fr")]
        #[case("es")]
        #[case("it")]
        fn should_render_partner_application_rejection_template_without_user_first_name_for(
            #[case] lang: &str,
        ) {
            let template_path = format!("mjml/partner-application/rejection/{lang}.mjml");
            let template = load_template(&template_path);
            let notification = make_partner_application_rejection_notification();
            let language = language_for_code(lang);
            let data = build_email_template_data(&notification, &language, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars
                .render_template(&template, &data)
                .unwrap_or_else(|e| panic!("Handlebars failed for {template_path}: {e}"));

            assert_no_unreplaced_handlebars(&rendered, &template_path);
            assert!(
                !rendered.contains("Thomas"),
                "Template should not contain user first name when not provided"
            );
        }

        #[test]
        fn should_include_shop_name_in_partner_application_approval_template() {
            let template_path = "mjml/partner-application/approval/en.mjml";
            let template = load_template(template_path);
            let notification = make_partner_application_approval_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains("Heritage Antiques"),
                "Rendered approval template should contain shop name"
            );
        }

        #[test]
        fn should_include_shop_name_in_partner_application_rejection_template() {
            let template_path = "mjml/partner-application/rejection/en.mjml";
            let template = load_template(template_path);
            let notification = make_partner_application_rejection_notification();
            let data =
                build_email_template_data(&notification, &Language::En, &Currency::Eur, None);

            let handlebars = Handlebars::new();
            let rendered = handlebars.render_template(&template, &data).unwrap();

            assert!(
                rendered.contains("Heritage Antiques"),
                "Rendered rejection template should contain shop name"
            );
        }
    }
}
