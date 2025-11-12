use common::{
    currency::domain::Currency,
    language::{data::LanguageData, domain::Language},
    price::domain::Price,
};
use mail_core::{
    payload::MailPayload,
    template::{MailTemplate, MailTemplateType},
};
use product::core::{
    product::Product,
    product_event::{ProductCommonEventPayload, ProductEvent, ProductEventPayload},
};
use product::service::get_service::{GetProductError, GetProductService};
use product::watchlist::service::product_watchlist_service::{
    ProductWatchListService, WatchProductError,
};
use serde_email::Email;
use serde_json::json;
use user::core::user::User;

#[derive(Debug, thiserror::Error)]
pub enum ProductEventMailPayloadServiceError {
    #[error("WatchProductError: {0}")]
    WatchProductError(#[from] WatchProductError),

    #[error("GetProductError: {0}")]
    GetProductError(#[from] GetProductError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductEventMailPayloadService {
    async fn create_mail_payloads(
        &self,
        event: ProductEvent,
    ) -> Result<Vec<MailPayload>, ProductEventMailPayloadServiceError>;
}

pub struct ItemEventMailPayloadServiceImpl<'a> {
    watchlist_service: &'a (dyn ProductWatchListService + Sync),
    get_item_service: &'a (dyn GetProductService + Sync),
    sender_email: Email,
}

impl<'a> ItemEventMailPayloadServiceImpl<'a> {
    pub fn new(
        watchlist_service: &'a (dyn ProductWatchListService + Sync),
        get_item_service: &'a (dyn GetProductService + Sync),
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
impl<'a> ProductEventMailPayloadService for ItemEventMailPayloadServiceImpl<'a> {
    async fn create_mail_payloads(
        &self,
        event: ProductEvent,
    ) -> Result<Vec<MailPayload>, ProductEventMailPayloadServiceError> {
        let users = self
            .watchlist_service
            .find_users_with_notifications(&event.aggregate_id)
            .await?;
        if users.is_empty() {
            return Ok(vec![]);
        }

        let item = self
            .get_item_service
            .find_item(event.payload.shop_id(), event.payload.shops_product_id())
            .await?;

        let mail_payloads = users
            .into_iter()
            .map(|user| self.customize_mail(user, &item, &event))
            .collect();
        Ok(mail_payloads)
    }
}

impl<'a> ItemEventMailPayloadServiceImpl<'a> {
    fn customize_mail(&self, user: User, item: &Product, event: &ProductEvent) -> MailPayload {
        // Defaulting to German/EUR now because UserRecord doesn't contain preferences yet
        let title = item
            .other_title
            .get(&Language::De)
            .unwrap_or(&item.native_title.payload);

        let subject = match event.payload {
            ProductEventPayload::Created(_) => {
                format!("Neue Antiquität: {}", title)
            }
            ProductEventPayload::StateListed(_) => {
                format!("Antiquität gelistet: {}", title)
            }
            ProductEventPayload::StateAvailable(_) => {
                format!("Antiquität verfügbar: {}", title)
            }
            ProductEventPayload::StateReserved(_) => {
                format!("Antiquität reserviert: {}", title)
            }
            ProductEventPayload::StateSold(_) => {
                format!("Antiquität verkauft: {}", title)
            }
            ProductEventPayload::StateRemoved(_) => {
                format!("Antiquität entfernt: {}", title)
            }
            ProductEventPayload::StateUnknown(_) => {
                format!("Zustand der Antiquität ist jetzt unbekannt: {}", title)
            }
            ProductEventPayload::PriceDiscovered(_) => {
                format!("Antiquität hat jetzt einen Preis: {}", title)
            }
            ProductEventPayload::PriceDropped(_) => {
                format!("Antiquität ist im Preis gefallen: {}", title)
            }
            ProductEventPayload::PriceIncreased(_) => {
                format!("Antiquität ist im Preis gestiegen: {}", title)
            }
            ProductEventPayload::PriceRemoved(_) => {
                format!("Preis beobachteter Antiquität wurde entfernt: {}", title)
            }
        };

        let template = match event.payload {
            ProductEventPayload::Created(_) => MailTemplate {
                template_type: MailTemplateType::CreatedNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::StateListed(_) => MailTemplate {
                template_type: MailTemplateType::StateListedNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::StateAvailable(_) => MailTemplate {
                template_type: MailTemplateType::StateAvailableNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::StateReserved(_) => MailTemplate {
                template_type: MailTemplateType::StateReservedNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::StateSold(_) => MailTemplate {
                template_type: MailTemplateType::StateSoldNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::StateRemoved(_) => MailTemplate {
                template_type: MailTemplateType::StateRemovedNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::StateUnknown(_) => MailTemplate {
                template_type: MailTemplateType::StateUnknownNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::PriceDiscovered(_) => MailTemplate {
                template_type: MailTemplateType::PriceDiscoveredNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::PriceDropped(_) => MailTemplate {
                template_type: MailTemplateType::PriceDroppedNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::PriceIncreased(_) => MailTemplate {
                template_type: MailTemplateType::PriceIncreasedNotification,
                language: LanguageData::De,
            },
            ProductEventPayload::PriceRemoved(_) => MailTemplate {
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
