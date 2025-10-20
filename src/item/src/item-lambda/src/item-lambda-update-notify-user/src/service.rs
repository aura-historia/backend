use common::{
    currency::domain::Currency,
    language::{data::LanguageData, domain::Language},
    price::domain::Price,
};
use item_core::{
    item::Item,
    item_event::{ItemCommonEventPayload, ItemEvent, ItemEventPayload},
};
use item_service::get_service::{GetItemError, GetItemService};
use item_watchlist::service::{ItemWatchListService, WatchItemError};
use mail_core::{
    payload::MailPayload,
    template::{MailTemplate, MailTemplateType},
};
use serde_email::Email;
use serde_json::json;
use user_core::user::User;

#[derive(Debug, thiserror::Error)]
pub enum ItemEventMailPayloadServiceError {
    #[error("WatchItemError: {0}")]
    WatchItemError(#[from] WatchItemError),

    #[error("GetItemError: {0}")]
    GetItemError(#[from] GetItemError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ItemEventMailPayloadService {
    async fn create_mail_payloads(
        &self,
        event: ItemEvent,
    ) -> Result<Vec<MailPayload>, ItemEventMailPayloadServiceError>;
}

pub struct ItemEventMailPayloadServiceImpl<'a> {
    watchlist_service: &'a (dyn ItemWatchListService + Sync),
    get_item_service: &'a (dyn GetItemService + Sync),
    sender_email: Email,
}

impl<'a> ItemEventMailPayloadServiceImpl<'a> {
    pub fn new(
        watchlist_service: &'a (dyn ItemWatchListService + Sync),
        get_item_service: &'a (dyn GetItemService + Sync),
        sender_email: Email,
    ) -> Self {
        ItemEventMailPayloadServiceImpl {
            watchlist_service,
            get_item_service,
            sender_email,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ItemEventMailPayloadService for ItemEventMailPayloadServiceImpl<'a> {
    async fn create_mail_payloads(
        &self,
        event: ItemEvent,
    ) -> Result<Vec<MailPayload>, ItemEventMailPayloadServiceError> {
        let users = self
            .watchlist_service
            .find_users_with_notifications(&event.aggregate_id)
            .await?;
        if users.is_empty() {
            return Ok(vec![]);
        }

        let item = self
            .get_item_service
            .find_item(event.payload.shop_id(), event.payload.shops_item_id())
            .await?;

        let mail_payloads = users
            .into_iter()
            .map(|user| self.customize_mail(user, &item, &event))
            .collect();
        Ok(mail_payloads)
    }
}

impl<'a> ItemEventMailPayloadServiceImpl<'a> {
    fn customize_mail(&self, user: User, item: &Item, event: &ItemEvent) -> MailPayload {
        // Defaulting to German/EUR now because UserRecord doesn't contain preferences yet
        let title = item
            .other_title
            .get(&Language::De)
            .unwrap_or(&item.native_title.payload);

        let subject = match event.payload {
            ItemEventPayload::Created(_) => {
                format!("Neue Antiquität: {}", title)
            }
            ItemEventPayload::StateListed(_) => {
                format!("Antiquität gelistet: {}", title)
            }
            ItemEventPayload::StateAvailable(_) => {
                format!("Antiquität verfügbar: {}", title)
            }
            ItemEventPayload::StateReserved(_) => {
                format!("Antiquität reserviert: {}", title)
            }
            ItemEventPayload::StateSold(_) => {
                format!("Antiquität verkauft: {}", title)
            }
            ItemEventPayload::StateRemoved(_) => {
                format!("Antiquität entfernt: {}", title)
            }
            ItemEventPayload::StateUnknown(_) => {
                format!("Zustand der Antiquität ist jetzt unbekannt: {}", title)
            }
            ItemEventPayload::PriceDiscovered(_) => {
                format!("Antiquität hat jetzt einen Preis: {}", title)
            }
            ItemEventPayload::PriceDropped(_) => {
                format!("Antiquität ist im Preis gefallen: {}", title)
            }
            ItemEventPayload::PriceIncreased(_) => {
                format!("Antiquität ist im Preis gestiegen: {}", title)
            }
            ItemEventPayload::PriceRemoved(_) => {
                format!("Preis beobachteter Antiquität wurde entfernt: {}", title)
            }
        };

        let template = match event.payload {
            ItemEventPayload::Created(_) => MailTemplate {
                template_type: MailTemplateType::CreatedNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::StateListed(_) => MailTemplate {
                template_type: MailTemplateType::StateListedNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::StateAvailable(_) => MailTemplate {
                template_type: MailTemplateType::StateAvailableNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::StateReserved(_) => MailTemplate {
                template_type: MailTemplateType::StateReservedNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::StateSold(_) => MailTemplate {
                template_type: MailTemplateType::StateSoldNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::StateRemoved(_) => MailTemplate {
                template_type: MailTemplateType::StateRemovedNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::StateUnknown(_) => MailTemplate {
                template_type: MailTemplateType::StateUnknownNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::PriceDiscovered(_) => MailTemplate {
                template_type: MailTemplateType::PriceDiscoveredNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::PriceDropped(_) => MailTemplate {
                template_type: MailTemplateType::PriceDroppedNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::PriceIncreased(_) => MailTemplate {
                template_type: MailTemplateType::PriceIncreasedNotification,
                language: LanguageData::De,
            },
            ItemEventPayload::PriceRemoved(_) => MailTemplate {
                template_type: MailTemplateType::PriceRemovedNotification,
                language: LanguageData::De,
            },
        };

        let mut data = json!({
            "title": title.to_string()
        });
        let data_ref = data.as_object_mut().expect(
            "shouldn't fail because it's initialized above as an object and not modified since",
        );

        if let Some(price_payload) = event.payload.as_price_changed() {
            let (currency, amount) = price_payload
                .new_other_price
                .get_key_value(&Currency::Eur)
                .unwrap_or((
                    &price_payload.new_native_price.currency,
                    &price_payload.new_native_price.monetary_amount,
                ));
            let price = Price::new(*amount, *currency);
            data_ref.insert("price".to_owned(), json!(price.format_human_readable()));
        }

        MailPayload {
            sender: self.sender_email.clone(),
            recipient: user.email,
            subject,
            template,
            data,
        }
    }
}
