use super::{SqlxListingSourceReaders, invalid_read, read_error};
use listing_source_core::{Domain, ListingSourceId};
use listing_source_service::ports::{ListingSourceReadError, ShopifySource, ShopifySourceReader};
use localization::Language;
use money::Currency;
use party_core::party_id::PartyId;

#[derive(sqlx::FromRow)]
struct ShopifyRow {
    listing_source_id: uuid::Uuid,
    operator_party_id: uuid::Uuid,
    domain: String,
    currency: Option<String>,
    language: Option<String>,
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

#[async_trait::async_trait]
impl ShopifySourceReader for SqlxListingSourceReaders {
    async fn find_by_domain(
        &self,
        domain: &Domain,
    ) -> Result<Option<ShopifySource>, ListingSourceReadError> {
        sqlx::query_as::<_, ShopifyRow>("SELECT c.listing_source_id,s.operator_party_id,c.domain,c.currency,c.language FROM listing_source_shopify_configurations c JOIN listing_sources s ON s.listing_source_id=c.listing_source_id WHERE c.domain=$1")
            .bind(domain.as_str()).fetch_optional(&self.pool).await.map_err(read_error)?
            .map(|row| Ok(ShopifySource {
                listing_source_id: ListingSourceId::from(row.listing_source_id),
                operator_party_id: PartyId::from(row.operator_party_id),
                domain: Domain::try_from(row.domain).map_err(invalid_read)?,
                currency: parse_optional_currency(row.currency.as_deref())?,
                language: parse_optional_language(row.language.as_deref())?,
            })).transpose()
    }
}
