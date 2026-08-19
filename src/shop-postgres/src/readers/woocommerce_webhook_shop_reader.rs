use common::error::boxed::box_error;
use localization::Language;
use money::Currency;
use platform_postgres::SqlxTransaction;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_id::ShopId;
use shop_service::ports::{
    WoocommerceWebhookShop, WoocommerceWebhookShopReadError, WoocommerceWebhookShopReader,
    WoocommerceWebhookShopReaderFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxWoocommerceWebhookShopReaderFactory;

struct SqlxWoocommerceWebhookShopReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct WoocommerceWebhookShopRow {
    shop_id: uuid::Uuid,
    partner_status: String,
    woocommerce_currency: Option<String>,
    woocommerce_language: Option<String>,
}

impl SqlxWoocommerceWebhookShopReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl WoocommerceWebhookShopReaderFactory<SqlxTransaction>
    for SqlxWoocommerceWebhookShopReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl WoocommerceWebhookShopReader + 'tx {
        SqlxWoocommerceWebhookShopReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl WoocommerceWebhookShopReader for SqlxWoocommerceWebhookShopReader<'_> {
    async fn find_for_webhook(
        &mut self,
        shop_id: ShopId,
    ) -> Result<Option<WoocommerceWebhookShop>, WoocommerceWebhookShopReadError> {
        sqlx::query_as::<_, WoocommerceWebhookShopRow>(
            r#"
            SELECT
                shop_id,
                partner_status,
                woocommerce_currency,
                woocommerce_language
            FROM shops
            WHERE shop_id = $1
            "#,
        )
        .bind(uuid::Uuid::from(shop_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(WoocommerceWebhookShopSqlxError)?
        .map(TryInto::try_into)
        .transpose()
        .map_err(|source| WoocommerceWebhookShopReadError::InvalidReadModel {
            source: box_error(source),
        })
    }
}

impl TryFrom<WoocommerceWebhookShopRow> for WoocommerceWebhookShop {
    type Error = WoocommerceWebhookShopRowMappingError;

    fn try_from(row: WoocommerceWebhookShopRow) -> Result<Self, Self::Error> {
        let partner_status = match row.partner_status.as_str() {
            "SCRAPED" => ShopPartnerStatus::Scraped,
            "PARTNERED" => ShopPartnerStatus::Partnered,
            _ => return Err(WoocommerceWebhookShopRowMappingError::PartnerStatus),
        };
        let currency = row
            .woocommerce_currency
            .as_deref()
            .map(parse_currency)
            .transpose()?;
        let language = row
            .woocommerce_language
            .as_deref()
            .map(parse_language)
            .transpose()?;
        Ok(Self {
            shop_id: ShopId::from(row.shop_id),
            partner_status,
            currency,
            language,
        })
    }
}

#[derive(Debug, thiserror::Error)]
enum WoocommerceWebhookShopRowMappingError {
    #[error("shop partner status persisted is invalid")]
    PartnerStatus,
    #[error("WooCommerce currency persisted is invalid")]
    Currency,
    #[error("WooCommerce language persisted is invalid")]
    Language,
}

fn parse_currency(value: &str) -> Result<Currency, WoocommerceWebhookShopRowMappingError> {
    match value.to_ascii_uppercase().as_str() {
        "EUR" => Ok(Currency::Eur),
        "GBP" => Ok(Currency::Gbp),
        "USD" => Ok(Currency::Usd),
        "AUD" => Ok(Currency::Aud),
        "CAD" => Ok(Currency::Cad),
        "NZD" => Ok(Currency::Nzd),
        "CNY" => Ok(Currency::Cny),
        "BRL" => Ok(Currency::Brl),
        "PLN" => Ok(Currency::Pln),
        "TRY" => Ok(Currency::Try),
        "JPY" => Ok(Currency::Jpy),
        "CZK" => Ok(Currency::Czk),
        "RUB" => Ok(Currency::Rub),
        "AED" => Ok(Currency::Aed),
        "SAR" => Ok(Currency::Sar),
        "HKD" => Ok(Currency::Hkd),
        "SGD" => Ok(Currency::Sgd),
        "CHF" => Ok(Currency::Chf),
        _ => Err(WoocommerceWebhookShopRowMappingError::Currency),
    }
}

fn parse_language(value: &str) -> Result<Language, WoocommerceWebhookShopRowMappingError> {
    match value.to_ascii_lowercase().as_str() {
        "de" => Ok(Language::De),
        "en" => Ok(Language::En),
        "fr" => Ok(Language::Fr),
        "es" => Ok(Language::Es),
        "it" => Ok(Language::It),
        "zh" => Ok(Language::Zh),
        "pt" => Ok(Language::Pt),
        "pl" => Ok(Language::Pl),
        "tr" => Ok(Language::Tr),
        "nl" => Ok(Language::Nl),
        "cs" => Ok(Language::Cs),
        "ja" => Ok(Language::Ja),
        "ru" => Ok(Language::Ru),
        "ar" => Ok(Language::Ar),
        _ => Err(WoocommerceWebhookShopRowMappingError::Language),
    }
}

struct WoocommerceWebhookShopSqlxError(sqlx::Error);

impl From<WoocommerceWebhookShopSqlxError> for WoocommerceWebhookShopReadError {
    fn from(error: WoocommerceWebhookShopSqlxError) -> Self {
        Self::TemporarilyUnavailable {
            source: box_error(error.0),
        }
    }
}
