use application::error::box_error;
use listing_source_core::{Domain, ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::ports::*;
use localization::Language;
use money::Currency;
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;

#[derive(Clone)]
pub struct SqlxListingSourceReaders {
    pool: PgPool,
}
impl SqlxListingSourceReaders {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
fn read_error(error: sqlx::Error) -> ListingSourceReadError {
    ListingSourceReadError::TemporarilyUnavailable {
        source: box_error(error),
    }
}
#[derive(sqlx::FromRow)]
struct DetailRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    name: String,
    operator_party_id: uuid::Uuid,
    party_slug_id: String,
    operator_name: String,
    methods: Vec<String>,
    url: Option<String>,
    image: Option<String>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}
fn invalid_read(error: impl std::error::Error + Send + Sync + 'static) -> ListingSourceReadError {
    ListingSourceReadError::InvalidReadModel {
        source: box_error(error),
    }
}

fn parse_optional_currency(
    value: Option<&str>,
) -> Result<Option<Currency>, ListingSourceReadError> {
    value
        .map(|currency| {
            Currency::from_code(currency).ok_or_else(|| {
                invalid_read(std::io::Error::other(
                    "persisted listing source currency is invalid",
                ))
            })
        })
        .transpose()
}

fn parse_optional_language(
    value: Option<&str>,
) -> Result<Option<Language>, ListingSourceReadError> {
    value
        .map(|language| {
            Language::from_code(language).ok_or_else(|| {
                invalid_read(std::io::Error::other(
                    "persisted listing source language is invalid",
                ))
            })
        })
        .transpose()
}

fn detail(row: DetailRow) -> Result<ListingSourceDetails, ListingSourceReadError> {
    Ok(ListingSourceDetails {
        listing_source_id: ListingSourceId::from(row.listing_source_id),
        slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        name: ListingSourceName::from(row.name),
        operator_party_id: PartyId::from(row.operator_party_id),
        operator_slug_id: PartySlugId::raw(row.party_slug_id).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        operator_name: PartyName::from(row.operator_name),
        acquisition_methods: row
            .methods
            .into_iter()
            .map(|value| value.parse())
            .collect::<Result<_, _>>()
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?,
        url: row
            .url
            .map(|value| Url::parse(&value))
            .transpose()
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?,
        image: row
            .image
            .map(|value| Url::parse(&value))
            .transpose()
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?,
        created: row.created,
        updated: row.updated,
    })
}
const DETAIL_SQL: &str = "SELECT s.listing_source_id,s.listing_source_slug_id,s.name,s.operator_party_id,p.party_slug_id,p.name AS operator_name,array_agg(m.acquisition_method) AS methods,s.url,s.image,s.created,s.updated FROM listing_sources s JOIN parties p ON p.party_id=s.operator_party_id LEFT JOIN listing_source_acquisition_methods m ON m.listing_source_id=s.listing_source_id WHERE s.listing_source_id=$1 GROUP BY s.listing_source_id,p.party_id";
const DETAIL_BY_SLUG_SQL: &str = "SELECT s.listing_source_id,s.listing_source_slug_id,s.name,s.operator_party_id,p.party_slug_id,p.name AS operator_name,array_agg(m.acquisition_method) AS methods,s.url,s.image,s.created,s.updated FROM listing_sources s JOIN parties p ON p.party_id=s.operator_party_id LEFT JOIN listing_source_acquisition_methods m ON m.listing_source_id=s.listing_source_id WHERE s.listing_source_slug_id=$1 GROUP BY s.listing_source_id,p.party_id";
#[async_trait::async_trait]
impl ListingSourceDetailsReader for SqlxListingSourceReaders {
    async fn find_details_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError> {
        sqlx::query_as::<_, DetailRow>(DETAIL_SQL)
            .bind(uuid::Uuid::from(id))
            .fetch_optional(&self.pool)
            .await
            .map_err(read_error)?
            .map(detail)
            .transpose()
    }
    async fn find_details_by_slug(
        &self,
        slug: &ListingSourceSlugId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError> {
        sqlx::query_as::<_, DetailRow>(DETAIL_BY_SLUG_SQL)
            .bind(slug.as_ref())
            .fetch_optional(&self.pool)
            .await
            .map_err(read_error)?
            .map(detail)
            .transpose()
    }
}
#[derive(sqlx::FromRow)]
struct ShopifyRow {
    listing_source_id: uuid::Uuid,
    operator_party_id: uuid::Uuid,
    domain: String,
    currency: Option<String>,
    language: Option<String>,
}
#[async_trait::async_trait]
impl ShopifySourceReader for SqlxListingSourceReaders {
    async fn find_by_domain(
        &self,
        domain: &Domain,
    ) -> Result<Option<ShopifySource>, ListingSourceReadError> {
        sqlx::query_as::<_, ShopifyRow>("SELECT c.listing_source_id,s.operator_party_id,c.domain,c.currency,c.language FROM listing_source_shopify_configurations c JOIN listing_sources s ON s.listing_source_id=c.listing_source_id WHERE c.domain=$1")
            .bind(domain.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(read_error)?
            .map(|row| {
                Ok(ShopifySource {
                    listing_source_id: ListingSourceId::from(row.listing_source_id),
                    operator_party_id: PartyId::from(row.operator_party_id),
                    domain: Domain::try_from(row.domain).map_err(invalid_read)?,
                    currency: parse_optional_currency(row.currency.as_deref())?,
                    language: parse_optional_language(row.language.as_deref())?,
                })
            })
            .transpose()
    }
}
#[derive(sqlx::FromRow)]
struct WooRow {
    listing_source_id: uuid::Uuid,
    operator_party_id: uuid::Uuid,
    currency: Option<String>,
    language: Option<String>,
}
#[async_trait::async_trait]
impl WoocommerceSourceReader for SqlxListingSourceReaders {
    async fn find_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<WoocommerceSource>, ListingSourceReadError> {
        sqlx::query_as::<_, WooRow>("SELECT c.listing_source_id,s.operator_party_id,c.currency,c.language FROM listing_source_woocommerce_configurations c JOIN listing_sources s ON s.listing_source_id=c.listing_source_id WHERE c.listing_source_id=$1")
            .bind(uuid::Uuid::from(id))
            .fetch_optional(&self.pool)
            .await
            .map_err(read_error)?
            .map(|row| {
                Ok(WoocommerceSource {
                    listing_source_id: ListingSourceId::from(row.listing_source_id),
                    operator_party_id: PartyId::from(row.operator_party_id),
                    currency: parse_optional_currency(row.currency.as_deref())?,
                    language: parse_optional_language(row.language.as_deref())?,
                })
            })
            .transpose()
    }
}
#[async_trait::async_trait]
impl WebCrawlSourceReader for SqlxListingSourceReaders {
    async fn list_sources(&self) -> Result<Vec<WebCrawlSource>, ListingSourceReadError> {
        let rows=sqlx::query_as::<_,(uuid::Uuid,uuid::Uuid,String)>("SELECT s.listing_source_id,s.operator_party_id,s.url FROM listing_sources s JOIN listing_source_acquisition_methods m ON m.listing_source_id=s.listing_source_id WHERE m.acquisition_method='WEB_CRAWL' AND s.url IS NOT NULL").fetch_all(&self.pool).await.map_err(read_error)?;
        rows.into_iter()
            .map(|(id, party, url)| {
                Ok(WebCrawlSource {
                    listing_source_id: ListingSourceId::from(id),
                    operator_party_id: PartyId::from(party),
                    url: Url::parse(&url).map_err(|error| {
                        ListingSourceReadError::InvalidReadModel {
                            source: box_error(error),
                        }
                    })?,
                })
            })
            .collect()
    }
}
#[async_trait::async_trait]
impl WoocommerceSignatureVerifier for SqlxListingSourceReaders {
    async fn verify(
        &self,
        id: ListingSourceId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceSignatureVerification, ListingSourceReadError> {
        let secret=sqlx::query_scalar::<_,Option<String>>("SELECT webhook_secret FROM listing_source_woocommerce_configurations WHERE listing_source_id=$1").bind(uuid::Uuid::from(id)).fetch_optional(&self.pool).await.map_err(read_error)?.flatten();
        let Some(secret) = secret else {
            return Ok(WoocommerceSignatureVerification::SecretNotConfigured);
        };
        use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
        let key = PKey::hmac(secret.as_bytes()).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?;
        let mut signer = Signer::new(MessageDigest::sha256(), &key).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?;
        signer
            .update(body)
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?;
        let expected =
            signer
                .sign_to_vec()
                .map_err(|error| ListingSourceReadError::InvalidReadModel {
                    source: box_error(error),
                })?;
        Ok(
            if expected.len() == signature.len() && openssl::memcmp::eq(&expected, signature) {
                WoocommerceSignatureVerification::Valid
            } else {
                WoocommerceSignatureVerification::Invalid
            },
        )
    }
}
