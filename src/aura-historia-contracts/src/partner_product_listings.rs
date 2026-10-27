//! Source-keyed integration inputs shared by synchronous and asynchronous transports.
use crate::{
    patch_value::{PatchValue, clearable},
    product_listing_fields::{ProductListingAuctionData, auction_patch, product_images},
    product_listing_management::MutationInputError,
    values::{LocalizedTextData, PriceData, ProductListingPriceData},
};
use application::patch_field::PatchField;
use listing_source_core::ListingSourceId;
use money::Price;
use product_listing_core::{
    description::Description, listing_availability::ListingAvailability,
    product_listing::ProductListingPricing, product_listing_id::ProductListingKey,
    source_listing_id::SourceListingId, title::Title,
};
use product_listing_service::use_cases::{
    CreateProductListingCommand, UpdateProductListingCommand, UpsertProductListingCommand,
};
use schemars::JsonSchema;
use serde::Deserialize;
use url::Url;

pub const MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE: usize = product_listing_service::use_cases::commands::partner_product_listing_batches::MAX_PRODUCT_LISTING_BATCH_SIZE;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProductListingData {
    pub source_listing_id: String,
    #[serde(default)]
    pub title: Option<LocalizedTextData>,
    #[serde(default)]
    pub description: Option<LocalizedTextData>,
    #[serde(default)]
    pub price: Option<ProductListingPriceData>,
    #[serde(default)]
    pub price_estimate_min: Option<PriceData>,
    #[serde(default)]
    pub price_estimate_max: Option<PriceData>,
    #[serde(default, with = "crate::wire::listing::availability_option")]
    #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
    pub availability: Option<ListingAvailability>,
    #[schemars(schema_with = "crate::wire::schema::url")]
    pub url: Url,
    #[schemars(schema_with = "crate::product_listing_fields::image_urls_schema")]
    pub images: Vec<Url>,
    #[serde(default)]
    #[schemars(with = "ProductListingAuctionData")]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateProductListingData {
    pub source_listing_id: String,
    #[serde(default)]
    pub price: PatchValue<ProductListingPriceData>,
    #[serde(default)]
    pub price_estimate_min: PatchValue<PriceData>,
    #[serde(default)]
    pub price_estimate_max: PatchValue<PriceData>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::listing::availability_patch")]
    #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
    pub availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::url")]
    pub url: PatchValue<Url>,
    #[serde(default)]
    #[schemars(schema_with = "crate::product_listing_fields::image_urls_schema")]
    pub images: PatchValue<Vec<Url>>,
    #[serde(default)]
    #[schemars(with = "ProductListingAuctionData")]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpsertProductListingData {
    pub source_listing_id: String,
    #[serde(default)]
    pub title: Option<LocalizedTextData>,
    #[serde(default)]
    pub description: Option<LocalizedTextData>,
    #[serde(default)]
    pub price: PatchValue<ProductListingPriceData>,
    #[serde(default)]
    pub price_estimate_min: PatchValue<PriceData>,
    #[serde(default)]
    pub price_estimate_max: PatchValue<PriceData>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::listing::availability_patch")]
    #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
    pub availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::optional_url")]
    pub url: Option<Url>,
    #[serde(default)]
    #[schemars(schema_with = "crate::product_listing_fields::image_urls_schema")]
    pub images: PatchValue<Vec<Url>>,
    #[serde(default)]
    #[schemars(with = "ProductListingAuctionData")]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawProductListingData {
    pub source_listing_id: String,
}

impl CreateProductListingData {
    pub fn into_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<CreateProductListingCommand, MutationInputError> {
        let auction = match auction_patch(self.auction)? {
            PatchField::Set(auction) => Some(auction),
            PatchField::Clear | PatchField::Unchanged => None,
        };
        Ok(CreateProductListingCommand {
            listing_source_id,
            source_listing_id: source_listing_id(self.source_listing_id)?,
            title: self.title.map(title).transpose()?,
            description: self.description.map(description),
            pricing: ProductListingPricing {
                price: self.price.map(product_listing_price),
                price_estimate_min: self.price_estimate_min.map(price),
                price_estimate_max: self.price_estimate_max.map(price),
            },
            availability: self.availability,
            url: self.url,
            images: product_images(self.images),
            auction,
        })
    }
}

impl UpdateProductListingData {
    pub fn into_key_and_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<(ProductListingKey, UpdateProductListingCommand), MutationInputError> {
        let product_key = ProductListingKey::new(
            listing_source_id,
            source_listing_id(self.source_listing_id)?,
        );
        let auction = auction_patch(self.auction)?;
        let command = UpdateProductListingCommand {
            price: clearable(self.price.map(product_listing_price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: non_nullable_patch(self.url, "url")?,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction,
        };
        Ok((product_key, command))
    }
}

impl UpsertProductListingData {
    pub fn into_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<UpsertProductListingCommand, MutationInputError> {
        let auction = auction_patch(self.auction)?;
        Ok(UpsertProductListingCommand {
            listing_source_id,
            source_listing_id: source_listing_id(self.source_listing_id)?,
            title: self.title.map(title).transpose()?,
            description: self.description.map(description),
            price: clearable(self.price.map(product_listing_price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: self.url,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction,
        })
    }
}

impl WithdrawProductListingData {
    pub fn into_product_key(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<ProductListingKey, MutationInputError> {
        Ok(ProductListingKey::new(
            listing_source_id,
            source_listing_id(self.source_listing_id)?,
        ))
    }
}

/// Rejects a title without text after normalization, such as a blank or emoji-only title.
fn title(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, Title>, MutationInputError> {
    let title: localization::Localized<localization::Language, Title> = value.into_localized();
    if title.payload.as_ref().is_empty() {
        return Err(MutationInputError::invalid(
            "title.text",
            "Title must contain text other than whitespace, emoji or symbols.",
        ));
    }
    Ok(title)
}

fn description(
    value: LocalizedTextData,
) -> localization::Localized<localization::Language, Description> {
    value.into_localized()
}

fn price(value: PriceData) -> Price {
    value.into()
}

fn product_listing_price(
    value: ProductListingPriceData,
) -> product_listing_core::product_listing_price::ProductListingPrice {
    value.into()
}

fn source_listing_id(value: String) -> Result<SourceListingId, MutationInputError> {
    SourceListingId::try_from(value)
        .map_err(|error| MutationInputError::caused("sourceListingId", error.to_string(), error))
}

fn non_nullable_patch<T>(
    value: PatchValue<T>,
    field: &'static str,
) -> Result<PatchField<T>, MutationInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Value(value) => Ok(PatchField::Set(value)),
        PatchValue::Null => Err(MutationInputError::invalid(
            field,
            format!("Body field '{field}' must not be null."),
        )),
    }
}

pub trait PartnerProductListingInput {
    type Command;
    fn source_listing_id(&self) -> &str;
    fn into_batch_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<Self::Command, MutationInputError>;
}

pub struct PreparedProductListingBatch<C> {
    /// Original spellings remain available for transport reports; command keys use domain normalization.
    pub source_listing_ids: Vec<String>,
    pub commands: Vec<C>,
}

pub fn prepare_batch<T: PartnerProductListingInput>(
    listing_source_id: ListingSourceId,
    products: Vec<T>,
) -> Result<PreparedProductListingBatch<T::Command>, MutationInputError> {
    validate_batch_size(products.len())?;
    let source_listing_ids = products
        .iter()
        .map(|product| product.source_listing_id().to_owned())
        .collect();
    let commands = products
        .into_iter()
        .map(|product| product.into_batch_command(listing_source_id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PreparedProductListingBatch {
        source_listing_ids,
        commands,
    })
}

pub fn validate_batch_size(count: usize) -> Result<(), MutationInputError> {
    if count > MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE {
        return Err(MutationInputError::invalid(
            "products",
            format!(
                "Body cannot contain more than {MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE} products."
            ),
        ));
    }
    Ok(())
}

impl PartnerProductListingInput for CreateProductListingData {
    type Command = CreateProductListingCommand;
    fn source_listing_id(&self) -> &str {
        &self.source_listing_id
    }
    fn into_batch_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<Self::Command, MutationInputError> {
        self.into_command(listing_source_id)
    }
}

impl PartnerProductListingInput for UpdateProductListingData {
    type Command = (ProductListingKey, UpdateProductListingCommand);
    fn source_listing_id(&self) -> &str {
        &self.source_listing_id
    }
    fn into_batch_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<Self::Command, MutationInputError> {
        self.into_key_and_command(listing_source_id)
    }
}

impl PartnerProductListingInput for UpsertProductListingData {
    type Command = UpsertProductListingCommand;
    fn source_listing_id(&self) -> &str {
        &self.source_listing_id
    }
    fn into_batch_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<Self::Command, MutationInputError> {
        self.into_command(listing_source_id)
    }
}

impl PartnerProductListingInput for WithdrawProductListingData {
    type Command = ProductListingKey;
    fn source_listing_id(&self) -> &str {
        &self.source_listing_id
    }
    fn into_batch_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<Self::Command, MutationInputError> {
        self.into_product_key(listing_source_id)
    }
}

pub mod admission;

#[cfg(test)]
mod tests {
    use super::{
        CreateProductListingData, UpdateProductListingData, UpsertProductListingData,
        WithdrawProductListingData, source_listing_id,
    };
    use application::patch_field::PatchField;
    use listing_source_core::ListingSourceId;

    #[test]
    fn should_reject_null_auction_in_partner_writes() {
        let listing_source_id = ListingSourceId::new();
        let create: CreateProductListingData = serde_json::from_str(
            r#"{
                "sourceListingId":"SKU-1",
                "title":{"text":"Listing","language":"en"},
                "description":{"text":"Description","language":"en"},
                "url":"https://example.com/listing",
                "images":[],
                "auction":null
            }"#,
        )
        .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let upsert: UpsertProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
                .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        assert_eq!(
            "auction",
            create
                .into_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .field
        );
        assert_eq!(
            "auction",
            update
                .into_key_and_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .field
        );
        assert_eq!(
            "auction",
            upsert
                .into_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .field
        );
    }

    #[test]
    fn should_allow_optional_title_and_description_on_create() {
        let listing_source_id = ListingSourceId::new();
        let cases = [
            (
                r#"{"sourceListingId":"SKU-1","url":"https://example.com/listing","images":[]}"#,
                false,
                false,
            ),
            (
                r#"{"sourceListingId":"SKU-2","title":{"text":"Title","language":"en"},"url":"https://example.com/listing","images":[]}"#,
                true,
                false,
            ),
            (
                r#"{"sourceListingId":"SKU-3","description":{"text":"Description","language":"en"},"url":"https://example.com/listing","images":[]}"#,
                false,
                true,
            ),
            (
                r#"{"sourceListingId":"SKU-4","title":null,"description":null,"url":"https://example.com/listing","images":[]}"#,
                false,
                false,
            ),
        ];

        for (body, expects_title, expects_description) in cases {
            let data: CreateProductListingData = serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
            let command = data
                .into_command(listing_source_id)
                .unwrap_or_else(|error| panic!("valid create command: {error}"));
            assert_eq!(expects_title, command.title.is_some());
            assert_eq!(expects_description, command.description.is_some());
        }
    }

    #[test]
    fn should_normalize_decorative_titles_on_create_and_upsert() {
        let listing_source_id = ListingSourceId::new();
        let create: CreateProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","title":{"text":"𝐀𝐧𝐫𝐢𝐜𝐡𝐭𝐞 🌿✨Kirschbaum | 3360","language":"de"},"url":"https://example.com/listing","images":[]}"#,
        )
        .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
        let upsert: UpsertProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","title":{"text":"𝐀𝐧𝐫𝐢𝐜𝐡𝐭𝐞 🌿✨Kirschbaum | 3360","language":"de"}}"#,
        )
        .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        for title in [
            create
                .into_command(listing_source_id)
                .unwrap_or_else(|error| panic!("valid create command: {error}"))
                .title,
            upsert
                .into_command(listing_source_id)
                .unwrap_or_else(|error| panic!("valid upsert command: {error}"))
                .title,
        ] {
            assert_eq!(
                Some("Anrichte Kirschbaum | 3360"),
                title.as_ref().map(|title| title.payload.as_ref())
            );
        }
    }

    #[test]
    fn should_reject_titles_without_text_on_create_and_upsert() {
        let listing_source_id = ListingSourceId::new();
        for text in ["", "   ", "🌿✨", " ★ ✿ "] {
            let title = serde_json::json!({"text": text, "language": "en"});
            let create: CreateProductListingData = serde_json::from_value(serde_json::json!({
                "sourceListingId": "SKU-1",
                "title": title,
                "url": "https://example.com/listing",
                "images": []
            }))
            .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
            let upsert: UpsertProductListingData = serde_json::from_value(serde_json::json!({
                "sourceListingId": "SKU-1",
                "title": title
            }))
            .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

            for error in [
                create.into_command(listing_source_id).err(),
                upsert.into_command(listing_source_id).err(),
            ] {
                let error = error.unwrap_or_else(|| panic!("title {text:?} must fail"));
                assert_eq!("title.text", error.field);
            }
        }
    }

    #[test]
    fn should_map_existing_auction_id_and_listing_owned_leaf_patches() {
        let auction_id = auction_core::AuctionId::new();
        let data: UpdateProductListingData = serde_json::from_str(&format!(
            r#"{{"sourceListingId":"SKU-1","auction":{{"auctionId":"{auction_id}","lotNumber":null,"timing":{{"biddingOpens":null,"reportedClosedAt":"2026-05-01T12:00:00Z"}}}}}}"#
        ))
        .unwrap_or_else(|error| panic!("valid update JSON: {error}"));

        let (_, command) = data
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let PatchField::Set(auction) = command.auction else {
            panic!("auction patch should be asserted");
        };
        assert_eq!(PatchField::Set(auction_id), auction.auction_id);
        assert_eq!(PatchField::Clear, auction.lot_number);
        assert_eq!(PatchField::Clear, auction.bidding_opens);
        assert!(matches!(auction.reported_closed_at, PatchField::Set(_)));
    }

    #[test]
    fn should_reject_date_only_and_timezone_less_lot_timestamps() {
        for timestamp in ["2026-05-01", "2026-05-01T12:00:00"] {
            assert!(serde_json::from_str::<UpdateProductListingData>(&format!(
                r#"{{"sourceListingId":"SKU-1","auction":{{"timing":{{"biddingOpens":"{timestamp}"}}}}}}"#
            ))
            .is_err());
        }
    }

    #[test]
    fn should_clear_membership_with_null_auction_id_and_reject_retired_fields() {
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":{"auctionId":null}}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let (_, command) = update
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let PatchField::Set(auction) = command.auction else {
            panic!("auction patch should be asserted");
        };
        assert_eq!(PatchField::Clear, auction.auction_id);

        assert!(
            serde_json::from_str::<UpdateProductListingData>(
                r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":"sale-42"}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<UpdateProductListingData>(
                r#"{"sourceListingId":"SKU-1","auction":{"metadata":{"name":"sale"}}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn should_leave_auction_unchanged_when_partner_update_or_upsert_omits_it() {
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let upsert: UpsertProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
                .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        let (_, update) = update
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let upsert = upsert
            .into_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid upsert command: {error}"));

        assert_eq!(update.auction, PatchField::Unchanged);
        assert_eq!(upsert.auction, PatchField::Unchanged);
    }

    #[test]
    fn should_parse_source_listing_id_without_slugifying_it() {
        let data: WithdrawProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"\u2003SKU  #42/Blue\u2002"}"#)
                .unwrap_or_else(|error| panic!("valid request data: {error}"));

        let key = data
            .into_product_key(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

        assert_eq!(key.source_listing_id.as_ref(), "SKU  #42/Blue");
    }

    #[test]
    fn should_reject_blank_source_listing_id_at_api_mapping() {
        let error = source_listing_id("\u{2003}\t".to_owned())
            .err()
            .unwrap_or_else(|| panic!("blank source listing ID must fail"));

        assert_eq!(error.field, "sourceListingId");
    }
}

pub mod shopify_snapshot;
