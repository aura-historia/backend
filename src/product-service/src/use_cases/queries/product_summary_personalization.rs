use crate::ports::{ProductUserStateLookup, ProductUserStateReadError, ProductUserStateReader};
use crate::use_cases::queries::search_products::PersonalizedProductSummary;
use common::error::boxed::{BoxError, box_error};
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;

use notification_service::ports::product_notification_ids_reader::{
    ProductNotificationIdsReadError, ProductNotificationIdsReader,
};
use product_core::title::Title;
use product_core::user_state::NotificationUserState;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProductSummaryPersonalizationError {
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
    #[error("product notification read failed")]
    NotificationReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state is missing for product {product_id}")]
    UserStateMissing { product_id: ProductId },
    #[error("hidden product summary could not be constructed")]
    HiddenProductSummaryInvalid {
        #[source]
        source: BoxError,
    },
}

pub(crate) async fn hydrate_product_summaries<U, N>(
    products: &mut [PersonalizedProductSummary],
    user_id: UserId,
    user_states: &U,
    notifications: &N,
) -> Result<(), ProductSummaryPersonalizationError>
where
    U: ProductUserStateReader,
    N: ProductNotificationIdsReader,
{
    if products.is_empty() {
        return Ok(());
    }

    let lookup = ProductUserStateLookup {
        user_id,
        product_ids: products
            .iter()
            .map(|product| product.item.product_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
    };
    let user_states_future = async {
        user_states
            .find_for_user(&lookup)
            .await
            .map_err(ProductSummaryPersonalizationError::from)
    };
    let notification_ids_future = async {
        notifications
            .unseen_ids_for_products(user_id, &lookup.product_ids)
            .await
            .map_err(ProductSummaryPersonalizationError::from)
    };
    let (user_states, mut notification_ids) =
        tokio::try_join!(user_states_future, notification_ids_future)?;

    for product in products {
        let mut user_state = user_states.get(&product.item.product_id).cloned().ok_or(
            ProductSummaryPersonalizationError::UserStateMissing {
                product_id: product.item.product_id,
            },
        )?;
        user_state.notification = NotificationUserState {
            unseen_notification_ids: notification_ids
                .remove(&product.item.product_id)
                .unwrap_or_default(),
        };
        let hidden = user_state.search_filter.hidden;
        product.user_state = Some(user_state);
        if hidden {
            redact_hidden_product_summary(&mut product.item)?;
        }
    }

    Ok(())
}

fn redact_hidden_product_summary(
    product: &mut crate::use_cases::queries::search_products::ProductSummary,
) -> Result<(), ProductSummaryPersonalizationError> {
    let nil = uuid::Uuid::nil();
    let language = product
        .title
        .as_ref()
        .map(|title| title.localization)
        .unwrap_or(Language::En);
    let hidden_url = Url::parse("https://aura-historia.com/pricing").map_err(|source| {
        ProductSummaryPersonalizationError::HiddenProductSummaryInvalid {
            source: box_error(source),
        }
    })?;

    product.product_id = ProductId::from(nil);
    product.product_slug_id = ProductSlugId::from("Hidden");
    product.event_id = EventId::from(nil);
    product.shop_id = ShopId::from(nil);
    product.seller_id = ShopId::from(nil);
    product.shops_product_id = ShopsProductId::from(nil.to_string());
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
        Language::En => Title::from("Hidden Product Title"),
        Language::Fr => Title::from("Titre du produit masqué"),
        Language::Es => Title::from("Título de producto oculto"),
        Language::It => Title::from("Titolo del prodotto nascosto"),
        _ => Title::from("Hidden Product Title"),
    }
}

impl From<ProductUserStateReadError> for ProductSummaryPersonalizationError {
    fn from(error: ProductUserStateReadError) -> Self {
        match error {
            ProductUserStateReadError::QueryFailed { source } => {
                Self::UserStateQueryFailed { source }
            }
            ProductUserStateReadError::InvalidReadModel { source } => {
                Self::UserStateReadModelInvalid { source }
            }
        }
    }
}

impl From<ProductNotificationIdsReadError> for ProductSummaryPersonalizationError {
    fn from(error: ProductNotificationIdsReadError) -> Self {
        Self::NotificationReadFailed {
            source: box_error(error),
        }
    }
}
