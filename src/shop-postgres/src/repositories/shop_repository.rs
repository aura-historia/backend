use crate::mapping::{
    ShopRow, bind_affiliate_configuration, bind_country, bind_currency, bind_domains,
    bind_language, bind_partner_status, bind_shop_type, shop_columns, version_to_i64,
};
use common::postgres::SqlxTransaction;
use common::write_metadata::WriteMetadata;
use common::{shop_id::ShopId, shop_slug_id::ShopSlugId};
use shop_core::shop::Shop;
use shop_service::ports::{
    ShopRepository, ShopRepositoryError, ShopRepositoryFactory, ShopStorageVersion, VersionedShop,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxShopRepositoryFactory;

struct SqlxShopRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxShopRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ShopRepositoryFactory<SqlxTransaction> for SqlxShopRepositoryFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl ShopRepository + 'tx {
        SqlxShopRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ShopRepository for SqlxShopRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: ShopId,
    ) -> Result<Option<VersionedShop>, ShopRepositoryError> {
        let sql = format!("SELECT {} FROM shops WHERE shop_id = $1", shop_columns());
        let row = sqlx::query_as::<_, ShopRow>(&sql)
            .bind(uuid::Uuid::from(id))
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(map_lookup_error)?;

        row.map(VersionedShop::try_from)
            .transpose()
            .map_err(|_| ShopRepositoryError::InvalidPersistedState)
    }

    async fn find_by_slug(
        &mut self,
        slug_id: &ShopSlugId,
    ) -> Result<Option<VersionedShop>, ShopRepositoryError> {
        let sql = format!(
            "SELECT {} FROM shops WHERE shop_slug_id = $1",
            shop_columns()
        );
        let row = sqlx::query_as::<_, ShopRow>(&sql)
            .bind(slug_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(map_lookup_error)?;

        row.map(VersionedShop::try_from)
            .transpose()
            .map_err(|_| ShopRepositoryError::InvalidPersistedState)
    }

    async fn insert(
        &mut self,
        shop: &Shop,
        metadata: &WriteMetadata,
    ) -> Result<(), ShopRepositoryError> {
        let address = shop.address();
        let structured_address = address.map(|value| &value.structured);
        let contact = shop.contact();
        let shopify = shop.shopify();
        let woocommerce = shop.woocommerce();
        let presentation = shop.presentation();

        sqlx::query(
            r#"
            INSERT INTO shops (
                shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains,
                shopify_domain, shopify_currency, shopify_language,
                woocommerce_webhook_secret, woocommerce_currency, woocommerce_language,
                url, image,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region,
                structured_address_postal_code, structured_address_country,
                geo_address_lat, geo_address_lon, phone, email, affiliate_configuration,
                created_by, updated_by
            ) VALUES (
                $1, $2, $3, $4, $5, $6,
                $7, $8, $9,
                $10, $11, $12,
                $13, $14,
                $15, $16, $17, $18, $19, $20,
                $21, $22, $23, $24, $25,
                $26, $27
            )
            "#,
        )
        .bind(uuid::Uuid::from(shop.id()))
        .bind(shop.slug_id().as_ref())
        .bind(shop.name().as_ref())
        .bind(bind_shop_type(shop.shop_type()))
        .bind(bind_partner_status(shop.partner_status()))
        .bind(bind_domains(shop))
        .bind(shopify.map(|value| value.domain.as_str()))
        .bind(bind_currency(shopify.and_then(|value| value.currency)))
        .bind(bind_language(shopify.and_then(|value| value.language)))
        .bind(woocommerce.and_then(|value| value.webhook_secret.as_ref().map(AsRef::as_ref)))
        .bind(bind_currency(woocommerce.and_then(|value| value.currency)))
        .bind(bind_language(woocommerce.and_then(|value| value.language)))
        .bind(presentation.url.as_ref().map(UrlToString::url_to_string))
        .bind(presentation.image.as_ref().map(UrlToString::url_to_string))
        .bind(structured_address.and_then(|value| value.addressline.as_deref()))
        .bind(structured_address.and_then(|value| value.addressline_extra.as_deref()))
        .bind(structured_address.and_then(|value| value.locality.as_deref()))
        .bind(structured_address.and_then(|value| value.region.as_deref()))
        .bind(structured_address.and_then(|value| value.postal_code.as_deref()))
        .bind(bind_country(address))
        .bind(address.and_then(|value| value.geo.map(|geo| geo.lat)))
        .bind(address.and_then(|value| value.geo.map(|geo| geo.lon)))
        .bind(contact.phone.as_deref())
        .bind(contact.email.as_ref().map(EmailToString::email_to_string))
        .bind(bind_affiliate_configuration(shop.affiliate_configuration()))
        .bind(metadata.actor())
        .bind(metadata.actor())
        .execute(&mut *self.connection)
        .await
        .map_err(map_write_error)?;

        Ok(())
    }

    async fn update(
        &mut self,
        shop: &Shop,
        expected_version: ShopStorageVersion,
        metadata: &WriteMetadata,
    ) -> Result<(), ShopRepositoryError> {
        let address = shop.address();
        let structured_address = address.map(|value| &value.structured);
        let contact = shop.contact();
        let shopify = shop.shopify();
        let woocommerce = shop.woocommerce();
        let presentation = shop.presentation();

        let result = sqlx::query(
            r#"
            UPDATE shops
            SET
                shop_slug_id = $1,
                name = $2,
                shop_type = $3,
                partner_status = $4,
                shop_domains = $5,
                shopify_domain = $6,
                shopify_currency = $7,
                shopify_language = $8,
                woocommerce_webhook_secret = $9,
                woocommerce_currency = $10,
                woocommerce_language = $11,
                url = $12,
                image = $13,
                structured_address_addressline = $14,
                structured_address_addressline_extra = $15,
                structured_address_locality = $16,
                structured_address_region = $17,
                structured_address_postal_code = $18,
                structured_address_country = $19,
                geo_address_lat = $20,
                geo_address_lon = $21,
                phone = $22,
                email = $23,
                affiliate_configuration = $24,
                version = version + 1,
                updated_by = $25,
                updated = now()
            WHERE shop_id = $26 AND version = $27
            "#,
        )
        .bind(shop.slug_id().as_ref())
        .bind(shop.name().as_ref())
        .bind(bind_shop_type(shop.shop_type()))
        .bind(bind_partner_status(shop.partner_status()))
        .bind(bind_domains(shop))
        .bind(shopify.map(|value| value.domain.as_str()))
        .bind(bind_currency(shopify.and_then(|value| value.currency)))
        .bind(bind_language(shopify.and_then(|value| value.language)))
        .bind(woocommerce.and_then(|value| value.webhook_secret.as_ref().map(AsRef::as_ref)))
        .bind(bind_currency(woocommerce.and_then(|value| value.currency)))
        .bind(bind_language(woocommerce.and_then(|value| value.language)))
        .bind(presentation.url.as_ref().map(UrlToString::url_to_string))
        .bind(presentation.image.as_ref().map(UrlToString::url_to_string))
        .bind(structured_address.and_then(|value| value.addressline.as_deref()))
        .bind(structured_address.and_then(|value| value.addressline_extra.as_deref()))
        .bind(structured_address.and_then(|value| value.locality.as_deref()))
        .bind(structured_address.and_then(|value| value.region.as_deref()))
        .bind(structured_address.and_then(|value| value.postal_code.as_deref()))
        .bind(bind_country(address))
        .bind(address.and_then(|value| value.geo.map(|geo| geo.lat)))
        .bind(address.and_then(|value| value.geo.map(|geo| geo.lon)))
        .bind(contact.phone.as_deref())
        .bind(contact.email.as_ref().map(EmailToString::email_to_string))
        .bind(bind_affiliate_configuration(shop.affiliate_configuration()))
        .bind(metadata.actor())
        .bind(uuid::Uuid::from(shop.id()))
        .bind(version_to_i64(expected_version))
        .execute(&mut *self.connection)
        .await
        .map_err(map_write_error)?;

        if result.rows_affected() == 0 {
            return Err(ShopRepositoryError::ConcurrencyConflict);
        }

        Ok(())
    }
}

fn map_lookup_error(_error: sqlx::Error) -> ShopRepositoryError {
    ShopRepositoryError::TemporarilyUnavailable
}

fn map_write_error(error: sqlx::Error) -> ShopRepositoryError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.constraint() == Some("shops_shop_slug_id_key") =>
        {
            ShopRepositoryError::SlugConflict
        }
        _ => ShopRepositoryError::Internal,
    }
}

trait UrlToString {
    fn url_to_string(&self) -> String;
}

impl UrlToString for url::Url {
    fn url_to_string(&self) -> String {
        self.to_string()
    }
}

trait EmailToString {
    fn email_to_string(&self) -> String;
}

impl EmailToString for serde_email::Email {
    fn email_to_string(&self) -> String {
        self.as_str().to_owned()
    }
}
