use crate::ports::{
    ProductListingUserStateLookup, ProductListingUserStateReadError, ProductListingUserStateReader,
};
use crate::use_cases::queries::search_product_listings::PersonalizedProductListingSummary;
use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use localization::{Language, Localized};
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::product_state::ProductState;
use product_listing_core::shop_listing_id::ShopListingId;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use user_core::user_id::UserId;

use product_listing_core::title::Title;

use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProductListingSummaryPersonalizationError {
    #[error("product user state query failed")]
    UserStateQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state read model is invalid")]
    UserStateReadModelInvalid {
        #[source]
        source: BoxError,
    },

    #[error("product user state is missing for product {product_listing_id}")]
    UserStateMissing {
        product_listing_id: ProductListingId,
    },
    #[error("hidden product summary could not be constructed")]
    HiddenProductListingSummaryInvalid {
        #[source]
        source: BoxError,
    },
}

pub(crate) async fn hydrate_product_summaries<U>(
    products: &mut [PersonalizedProductListingSummary],
    user_id: UserId,
    user_states: &U,
) -> Result<(), ProductListingSummaryPersonalizationError>
where
    U: ProductListingUserStateReader,
{
    if products.is_empty() {
        return Ok(());
    }

    let lookup = ProductListingUserStateLookup {
        user_id,
        product_listing_ids: products
            .iter()
            .map(|product| product.item.product_listing_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
    };
    let user_states = user_states
        .find_for_user(&lookup)
        .await
        .map_err(ProductListingSummaryPersonalizationError::from)?;

    for product in products {
        let user_state = user_states
            .get(&product.item.product_listing_id)
            .cloned()
            .ok_or(
                ProductListingSummaryPersonalizationError::UserStateMissing {
                    product_listing_id: product.item.product_listing_id,
                },
            )?;

        let hidden = user_state.search_filter.hidden;
        product.user_state = Some(user_state);
        if hidden {
            redact_hidden_product_summary(&mut product.item)?;
        }
    }

    Ok(())
}

fn redact_hidden_product_summary(
    product: &mut crate::use_cases::queries::search_product_listings::ProductListingSummary,
) -> Result<(), ProductListingSummaryPersonalizationError> {
    let nil = uuid::Uuid::nil();
    let language = product
        .title
        .as_ref()
        .map(|title| title.localization)
        .unwrap_or(Language::En);
    let hidden_url = Url::parse("https://aura-historia.com/pricing").map_err(|source| {
        ProductListingSummaryPersonalizationError::HiddenProductListingSummaryInvalid {
            source: box_error(source),
        }
    })?;

    product.product_listing_id = ProductListingId::from(nil);
    product.product_listing_slug_id = ProductListingSlugId::from("Hidden");
    product.event_id = EventId::from(nil);
    product.shop_id = ShopId::from(nil);
    product.seller_id = ShopId::from(nil);
    product.shop_listing_id = ShopListingId::from(nil.to_string());
    product.shop_name = ShopName::from("Hidden");
    product.shop_slug_id = ShopSlugId::from("Hidden");
    product.title = Some(Localized::new(language, hidden_title(language)));
    product.display_price = None;
    product.state = ProductState::Unknown;
    product.lifecycle = ProductLifecycle::Active;
    product.url = hidden_url.clone();
    product.view_url = hidden_url;
    product.images.clear();
    product.updated = OffsetDateTime::UNIX_EPOCH;

    Ok(())
}

fn hidden_title(language: Language) -> Title {
    match language {
        Language::De => Title::from("Versteckter Produkttitel"),
        Language::En => Title::from("Hidden ProductListing Title"),
        Language::Fr => Title::from("Titre du produit masqué"),
        Language::Es => Title::from("Título de producto oculto"),
        Language::It => Title::from("Titolo del prodotto nascosto"),
        _ => Title::from("Hidden ProductListing Title"),
    }
}

impl From<ProductListingUserStateReadError> for ProductListingSummaryPersonalizationError {
    fn from(error: ProductListingUserStateReadError) -> Self {
        match error {
            ProductListingUserStateReadError::QueryFailed { source } => {
                Self::UserStateQueryFailed { source }
            }
            ProductListingUserStateReadError::InvalidReadModel { source } => {
                Self::UserStateReadModelInvalid { source }
            }
        }
    }
}
