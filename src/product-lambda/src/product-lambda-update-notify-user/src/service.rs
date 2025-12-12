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

pub struct ProductEventMailPayloadServiceImpl<'a> {
    watchlist_service: &'a (dyn ProductWatchListService + Sync),
    get_product_service: &'a (dyn GetProductService + Sync),
    sender_email: Email,
}

impl<'a> ProductEventMailPayloadServiceImpl<'a> {
    pub fn new(
        watchlist_service: &'a (dyn ProductWatchListService + Sync),
        get_product_service: &'a (dyn GetProductService + Sync),
        sender_email: Email,
    ) -> Self {
        ProductEventMailPayloadServiceImpl {
            watchlist_service,
            get_product_service,
            sender_email,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ProductEventMailPayloadService for ProductEventMailPayloadServiceImpl<'a> {
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

        let product = self
            .get_product_service
            .find_product(event.payload.shop_id(), event.payload.shops_product_id())
            .await?;

        let mail_payloads = users
            .into_iter()
            .map(|user| self.customize_mail(user, &product, &event))
            .collect();
        Ok(mail_payloads)
    }
}

impl<'a> ProductEventMailPayloadServiceImpl<'a> {
    fn customize_mail(&self, user: User, product: &Product, event: &ProductEvent) -> MailPayload {
        // Defaulting to English/EUR now due to lack of time for internationalized templates
        let title = product
            .other_title
            .get(&Language::En)
            .unwrap_or(&product.native_title.payload);

        let subject = "There's an update for one of the antiques on your watchlist!".to_owned();
        let template = resolve_mail_template(&event.payload, &user);

        let mut data = json!({
            "productUrl": product.url,
            "productTitle": title.to_string(),
            "productShopName": product.shop_name,
            "productShopId": product.shop_id,
            "productShopsProductId": product.shops_product_id,
        });
        let data_ref = data.as_object_mut().expect(
            "shouldn't fail because it's initialized above as an object and not modified since",
        );

        if let Some(user_first_name) = user.first_name {
            data_ref.insert("userFirstName".to_owned(), json!(user_first_name));
        }
        if let Some(user_last_name) = user.last_name {
            data_ref.insert("userLastName".to_owned(), json!(user_last_name));
        }

        if let Some(price_payload) = event.payload.as_price_changed() {
            let (old_currency, old_amount) = price_payload
                .old_other_price
                .get_key_value(user.currency.as_ref().unwrap_or(&Currency::default()))
                .unwrap_or((
                    &price_payload.old_native_price.currency,
                    &price_payload.old_native_price.monetary_amount,
                ));
            let old_price = Price::new(*old_amount, *old_currency);
            data_ref.insert(
                "productOldPrice".to_owned(),
                json!(old_price.format_human_readable()),
            );

            let (new_currency, new_amount) = price_payload
                .new_other_price
                .get_key_value(user.currency.as_ref().unwrap_or(&Currency::default()))
                .unwrap_or((
                    &price_payload.new_native_price.currency,
                    &price_payload.new_native_price.monetary_amount,
                ));
            let new_price = Price::new(*new_amount, *new_currency);
            data_ref.insert(
                "productNewPrice".to_owned(),
                json!(new_price.format_human_readable()),
            );
        }

        if let Some(state_payload) = event.payload.as_state_changed() {
            data_ref.insert(
                "productOldState".to_owned(),
                json!(state_payload.old_state.format_human_readable(&Language::En)),
            );
            data_ref.insert(
                "productNewState".to_owned(),
                json!(state_payload.old_state.format_human_readable(&Language::En)),
            );
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

fn resolve_mail_template(event_payload: &ProductEventPayload, _user: &User) -> MailTemplate {
    // Defaulting to English/EUR now due to lack of time for internationalized templates
    match event_payload {
        ProductEventPayload::Created(_) => MailTemplate {
            template_type: MailTemplateType::CreatedNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::StateListed(_) => MailTemplate {
            template_type: MailTemplateType::StateListedNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::StateAvailable(_) => MailTemplate {
            template_type: MailTemplateType::StateAvailableNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::StateReserved(_) => MailTemplate {
            template_type: MailTemplateType::StateReservedNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::StateSold(_) => MailTemplate {
            template_type: MailTemplateType::StateSoldNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::StateRemoved(_) => MailTemplate {
            template_type: MailTemplateType::StateRemovedNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::StateUnknown(_) => MailTemplate {
            template_type: MailTemplateType::StateUnknownNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::PriceDiscovered(_) => MailTemplate {
            template_type: MailTemplateType::PriceDiscoveredNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::PriceDropped(_) => MailTemplate {
            template_type: MailTemplateType::PriceDroppedNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::PriceIncreased(_) => MailTemplate {
            template_type: MailTemplateType::PriceIncreasedNotification,
            language: LanguageData::En,
        },
        ProductEventPayload::PriceRemoved(_) => MailTemplate {
            template_type: MailTemplateType::PriceRemovedNotification,
            language: LanguageData::En,
        },
    }
}
