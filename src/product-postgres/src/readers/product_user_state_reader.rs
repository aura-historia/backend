use application::error::{box_error, static_error};
use product_core::product_id::ProductId;
use product_service::ports::{
    ProductUserStateLookup, ProductUserStateReadError, ProductUserStateReader,
};
use product_service::user_state::{
    ProductUserState, ProhibitedContentUserState, SearchFilterUserState, WatchlistUserState,
};
use search_filter_core::enhanced_match_reason::EnhancedMatchReason;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use sqlx::PgPool;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SqlxProductUserStateReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductUserStateRow {
    product_id: Option<uuid::Uuid>,
    has_prohibited_content: Option<bool>,
    user_prohibited_content_consent: bool,
    user_tier: String,
    watchlist_notifications: Option<bool>,
    selected_match_user_search_filter_id: Option<uuid::Uuid>,
    selected_match_user_search_filter_name: Option<String>,
    selected_match_reason: Option<String>,
    selected_match_feedback: Option<bool>,
    selected_match_month_position: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
enum ProductUserStateRowMappingError {
    #[error("product user state row was returned without a requested product")]
    MissingRequestedProduct,
    #[error("product user state row is missing prohibited-content input")]
    MissingProhibitedContentInput,
    #[error("product user state has an invalid tier")]
    InvalidTier,
    #[error("unmatched product user state contains match fields")]
    UnmatchedProductHasMatchFields,
    #[error("free-tier matched product user state is missing monthly match position")]
    MissingFreeTierMonthPosition,
    #[error("unlimited-tier matched product user state has a monthly match position")]
    UnexpectedUnlimitedTierMonthPosition,
}

#[derive(Debug, Clone, Copy)]
enum UserTier {
    Free,
    Unlimited,
}

impl SqlxProductUserStateReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductUserStateReader for SqlxProductUserStateReader {
    async fn find_for_user(
        &self,
        lookup: &ProductUserStateLookup,
    ) -> Result<HashMap<ProductId, ProductUserState>, ProductUserStateReadError> {
        let product_ids = lookup
            .product_ids
            .iter()
            .copied()
            .map(uuid::Uuid::from)
            .collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, ProductUserStateRow>(SELECT_PRODUCT_USER_STATES)
            .bind(uuid::Uuid::from(lookup.user_id))
            .bind(product_ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| ProductUserStateReadError::QueryFailed {
                source: box_error(source),
            })?;

        if rows.is_empty() {
            return Err(ProductUserStateReadError::InvalidReadModel {
                source: static_error("product user state user is missing"),
            });
        }

        if lookup.product_ids.is_empty() {
            let [row] = rows.as_slice() else {
                return Err(ProductUserStateReadError::InvalidReadModel {
                    source: static_error("empty product user state lookup returned product rows"),
                });
            };
            user_tier(&row.user_tier).map_err(|source| {
                ProductUserStateReadError::InvalidReadModel {
                    source: box_error(source),
                }
            })?;

            if row.product_id.is_some() || row.has_prohibited_content.is_some() {
                return Err(ProductUserStateReadError::InvalidReadModel {
                    source: static_error("empty product user state lookup returned a product"),
                });
            }

            return Ok(HashMap::new());
        }

        rows.into_iter()
            .map(product_user_state)
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(|source| ProductUserStateReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}

const SELECT_PRODUCT_USER_STATES: &str = r#"
    WITH requested_products AS (
        SELECT DISTINCT requested.product_id
        FROM UNNEST($2::uuid[]) AS requested(product_id)
    ),
    requested_rows AS (
        SELECT product_id
        FROM requested_products

        UNION ALL

        SELECT NULL::uuid
        WHERE NOT EXISTS (SELECT 1 FROM requested_products)
    ),
    authenticated_user AS (
        SELECT prohibited_content_consent, tier
        FROM users
        WHERE user_id = $1
    ),
    ranked_requested_matches AS (
        SELECT
            matched.product_id,
            matched.user_search_filter_id,
            matched.user_search_filter_name,
            matched.enhanced_match_reason,
            matched.feedback,
            matched.created,
            ROW_NUMBER() OVER (
                PARTITION BY matched.product_id
                ORDER BY matched.created ASC, matched.user_search_filter_id ASC
            ) AS product_position
        FROM search_filter_matches matched
        JOIN requested_products requested
            ON requested.product_id = matched.product_id
        WHERE matched.user_id = $1
    ),
    selected_matches AS (
        SELECT
            product_id,
            user_search_filter_id,
            user_search_filter_name,
            enhanced_match_reason,
            feedback,
            created,
            date_trunc('month', created AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS month_start
        FROM ranked_requested_matches
        WHERE product_position = 1
    ),
    selected_match_months AS (
        SELECT DISTINCT month_start
        FROM selected_matches
    ),
    ranked_month_matches AS (
        SELECT
            matched.product_id,
            matched.user_search_filter_id,
            ROW_NUMBER() OVER (
                PARTITION BY date_trunc('month', matched.created AT TIME ZONE 'UTC')
                ORDER BY
                    matched.created ASC,
                    matched.user_search_filter_id ASC,
                    matched.product_id ASC
            ) AS month_position
        FROM authenticated_user authenticated_user
        JOIN search_filter_matches matched
            ON matched.user_id = $1
        JOIN selected_match_months month
            ON matched.created >= month.month_start
            AND matched.created < month.month_start + INTERVAL '1 month'
        WHERE authenticated_user.tier = 'FREE'
    )
    SELECT
        requested.product_id,
        CASE
            WHEN requested.product_id IS NULL OR product.product_id IS NULL THEN NULL
            ELSE EXISTS (
                SELECT 1
                FROM jsonb_array_elements(product.product_images) AS image
                WHERE image ->> 'prohibited_content' IS DISTINCT FROM 'NONE'
            )
        END AS has_prohibited_content,
        authenticated_user.prohibited_content_consent AS user_prohibited_content_consent,
        authenticated_user.tier AS user_tier,
        watchlist.notifications AS watchlist_notifications,
        selected_match.user_search_filter_id AS selected_match_user_search_filter_id,
        selected_match.user_search_filter_name AS selected_match_user_search_filter_name,
        selected_match.enhanced_match_reason AS selected_match_reason,
        selected_match.feedback AS selected_match_feedback,
        CASE
            WHEN authenticated_user.tier = 'FREE' THEN monthly_match.month_position
            ELSE NULL
        END AS selected_match_month_position
    FROM authenticated_user authenticated_user
    CROSS JOIN requested_rows requested
    LEFT JOIN products product
        ON product.product_id = requested.product_id
    LEFT JOIN product_watchlist watchlist
        ON watchlist.user_id = $1
        AND watchlist.product_id = requested.product_id
    LEFT JOIN selected_matches selected_match
        ON selected_match.product_id = requested.product_id
    LEFT JOIN ranked_month_matches monthly_match
        ON monthly_match.product_id = selected_match.product_id
        AND monthly_match.user_search_filter_id = selected_match.user_search_filter_id
"#;

fn product_user_state(
    row: ProductUserStateRow,
) -> Result<(ProductId, ProductUserState), ProductUserStateRowMappingError> {
    let product_id = row
        .product_id
        .map(ProductId::from)
        .ok_or(ProductUserStateRowMappingError::MissingRequestedProduct)?;
    let has_prohibited_content = row
        .has_prohibited_content
        .ok_or(ProductUserStateRowMappingError::MissingProhibitedContentInput)?;
    let tier = user_tier(&row.user_tier)?;
    let search_filter = search_filter_user_state(&row, tier)?;

    Ok((
        product_id,
        ProductUserState {
            watchlist: WatchlistUserState {
                watching: row.watchlist_notifications.is_some(),
                notifications: row.watchlist_notifications.unwrap_or(false),
            },
            prohibited_content: ProhibitedContentUserState {
                consent: !has_prohibited_content || row.user_prohibited_content_consent,
            },
            notification: Default::default(),
            search_filter,
        },
    ))
}

fn user_tier(value: &str) -> Result<UserTier, ProductUserStateRowMappingError> {
    match value {
        "FREE" => Ok(UserTier::Free),
        "PRO" | "ULTIMATE" => Ok(UserTier::Unlimited),
        _ => Err(ProductUserStateRowMappingError::InvalidTier),
    }
}

fn search_filter_user_state(
    row: &ProductUserStateRow,
    tier: UserTier,
) -> Result<SearchFilterUserState, ProductUserStateRowMappingError> {
    let Some(user_search_filter_id) = row.selected_match_user_search_filter_id else {
        if row.selected_match_user_search_filter_name.is_some()
            || row.selected_match_reason.is_some()
            || row.selected_match_feedback.is_some()
            || row.selected_match_month_position.is_some()
        {
            return Err(ProductUserStateRowMappingError::UnmatchedProductHasMatchFields);
        }

        return Ok(SearchFilterUserState::default());
    };

    let hidden = match tier {
        UserTier::Free => {
            row.selected_match_month_position
                .ok_or(ProductUserStateRowMappingError::MissingFreeTierMonthPosition)?
                > 10
        }
        UserTier::Unlimited => {
            if row.selected_match_month_position.is_some() {
                return Err(ProductUserStateRowMappingError::UnexpectedUnlimitedTierMonthPosition);
            }
            false
        }
    };

    Ok(SearchFilterUserState {
        matched: true,
        hidden,
        user_search_filter_id: Some(UserSearchFilterId::from(user_search_filter_id)),
        user_search_filter_name: row
            .selected_match_user_search_filter_name
            .clone()
            .map(UserSearchFilterName::from),
        match_reason: row
            .selected_match_reason
            .clone()
            .map(EnhancedMatchReason::from),
        match_feedback: row.selected_match_feedback,
    })
}
