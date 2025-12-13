use common::{currency::domain::Currency, language::domain::Language, price::domain::Price};
use mail_core::{
    payload::MailPayload,
    template::{MailTemplate, MailTemplateType},
};
use product::core::{
    product::Product,
    product_event::{ProductCommonEventPayload, ProductEvent},
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
        let title = product
            .other_title
            .get(&user.language.unwrap_or_default())
            .unwrap_or(&product.native_title.payload);

        // TODO
        // - i18n mjml template payload
        let subject = match user.language.unwrap_or_default() {
            Language::De => format!("Antiquitäten-Update für: {title}"),
            Language::En => format!("Antiques update on: {title}"),
            Language::Fr => format!("Mise à jour des antiquités : {title}"),
            Language::Es => format!("Actualización de antigüedades: {title}"),
        };

        let template = MailTemplate {
            template_type: MailTemplateType::WatchlistUpdate,
            language: user.language.unwrap_or_default().into(),
        };

        let mut data = json!({
            "product.auraHistoriaUrl": format!("https://aura-historia.com/product/{}/{}", product.shop_id, product.shops_product_id),
            "product.shopUrl": product.url,
            "product.title": title.to_string(),
            "product.shopName": product.shop_name,
        });
        let data_ref = data.as_object_mut().expect(
            "shouldn't fail because it's initialized above as an object and not modified since",
        );

        if let Some(user_first_name) = user.first_name {
            data_ref.insert("user.firstName".to_owned(), json!(user_first_name));
        }
        if let Some(user_last_name) = user.last_name {
            data_ref.insert("user.lastName".to_owned(), json!(user_last_name));
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
                "product.oldPrice".to_owned(),
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
                "product.newPrice".to_owned(),
                json!(new_price.format_human_readable()),
            );
        }

        if let Some(state_payload) = event.payload.as_state_changed() {
            data_ref.insert(
                "product.oldState".to_owned(),
                json!(
                    state_payload
                        .old_state
                        .format_human_readable(&user.language.unwrap_or_default())
                ),
            );
            data_ref.insert(
                "product.newState".to_owned(),
                json!(
                    state_payload
                        .old_state
                        .format_human_readable(&user.language.unwrap_or_default())
                ),
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
