use super::listing_row_values::{
    InvalidListingRowValue, availability, content_policy, images, lifecycle, localized_title,
    price, product_listing_price,
};
use crate::{
    listing_source_operator::listing_source_operator,
    object_id::{PersistedObjectIdError, try_from_uuid},
    product_listing_auction::{
        JoinedAuctionParts, ProductListingAuctionParts, auction_from_parts, map_joined_auction,
    },
    url::referral_configuration,
};
use application::error::box_error;
use application::personalized::Personalized;

use indexmap::IndexSet;
use listing_source_core::{ListingSourceName, ListingSourceSlugId, view_link};

use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing::{ListingSaleObservation, ProductListingPricing};

use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;

use product_listing_service::listing_text::ListingText;
use product_listing_service::ports::{
    ListingSourceSummary, PersonalizedProductListingDetailsReadModel,
    ProductListingDetailsReadError, ProductListingDetailsReadModel,
    ProductListingDetailsReadRequest, ProductListingDetailsReader,
    ProductListingDetailsReaderFactory,
};
use product_listing_service::use_cases::queries::get_product_listing::ProductListingLookup;
use product_listing_service::user_state::{
    ContentVisibilityUserState, NotificationUserState, ProductListingUserState,
    SearchFilterUserState, WatchlistUserState,
};
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use sqlx::{PgConnection, Postgres, QueryBuilder};

use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingDetailsReaderFactory;

struct SqlxProductListingDetailsReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct ProductListingDetailsRow {
    pub(super) product_listing_id: uuid::Uuid,
    product_listing_title_slug_id: String,
    current_event_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    source_listing_id: String,
    listing_source_name: String,
    listing_source_slug_id: String,
    listing_source_referral_configuration: Option<serde_json::Value>,
    listing_source_operator_name: String,
    listing_source_operator_partner: bool,
    product_title_text: Option<String>,
    product_title_language: Option<String>,
    title_text: Option<String>,
    title_language: Option<String>,
    price_kind: Option<String>,
    price_amount: Option<i64>,
    price_currency: Option<String>,
    price_estimate_min_amount: Option<i64>,
    price_estimate_min_currency: Option<String>,
    price_estimate_max_amount: Option<i64>,
    price_estimate_max_currency: Option<String>,
    sale_observation_fx_rate_id: Option<uuid::Uuid>,
    sale_observed_at: Option<OffsetDateTime>,
    availability: Option<String>,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    content_policy_decision: Option<String>,
    content_policy_category: Option<String>,
    auction_id: Option<uuid::Uuid>,
    lot_number: Option<String>,
    catalogue_position: Option<i64>,
    lot_bidding_opens_at: Option<OffsetDateTime>,
    lot_scheduled_closes_at: Option<OffsetDateTime>,
    lot_reported_closed_at: Option<OffsetDateTime>,
    parent_auction_id: Option<uuid::Uuid>,
    parent_listing_source_id: Option<uuid::Uuid>,
    auction_name_text: Option<String>,
    auction_name_language: Option<String>,
    auction_format: Option<String>,
    auction_bidding_opens_at: Option<OffsetDateTime>,
    auction_live_starts_at: Option<OffsetDateTime>,
    auction_lots_begin_closing_at: Option<OffsetDateTime>,
    auction_scheduled_end_at: Option<OffsetDateTime>,
    auction_reported_status: Option<String>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    personalization_user_id: Option<uuid::Uuid>,
    user_show_unassessed_or_sensitive_content: Option<bool>,
    user_tier: Option<String>,
    watchlist_notifications: Option<bool>,
    selected_match_user_search_filter_id: Option<uuid::Uuid>,
    selected_match_user_search_filter_name: Option<String>,
    selected_match_feedback: Option<bool>,
    selected_match_month_position: Option<i64>,
    unseen_notification_ids: Option<Vec<uuid::Uuid>>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ProductListingDetailsRowMappingError {
    #[error("product details row contains an invalid persisted object ID")]
    InvalidObjectId(#[source] PersistedObjectIdError),
    #[error("product details row contains an invalid persisted value")]
    InvalidValue(#[from] InvalidListingRowValue),
}

impl SqlxProductListingDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingDetailsReaderFactory<SqlxTransaction>
    for SqlxProductListingDetailsReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingDetailsReader + 'tx {
        SqlxProductListingDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingDetailsReader for SqlxProductListingDetailsReader<'_> {
    async fn find_details(
        &mut self,
        request: &ProductListingDetailsReadRequest,
    ) -> Result<Option<PersonalizedProductListingDetailsReadModel>, ProductListingDetailsReadError>
    {
        let requested_language = request.language.as_str();
        let user_id = request.user_id.map(|id| id.into_uuid());
        let row = match &request.lookup {
            ProductListingLookup::ById(product_listing_id) => {
                let mut query = QueryBuilder::<Postgres>::new(product_details_select(
                    DEFAULT_NOTIFICATION_STATES,
                ));
                query.push(" WHERE p.product_listing_id = $3");
                query
                    .build_query_as::<ProductListingDetailsRow>()
                    .bind(requested_language)
                    .bind(user_id)
                    .bind(product_listing_id.as_uuid())
                    .fetch_optional(&mut *self.connection)
                    .await
            }

            ProductListingLookup::ByTitleSlug(product_listing_title_slug_id) => {
                let mut query = QueryBuilder::<Postgres>::new(product_details_select(
                    DEFAULT_NOTIFICATION_STATES,
                ));
                query.push(" WHERE p.product_listing_title_slug_id = $3");
                query
                    .build_query_as::<ProductListingDetailsRow>()
                    .bind(requested_language)
                    .bind(user_id)
                    .bind(product_listing_title_slug_id.as_ref())
                    .fetch_optional(&mut *self.connection)
                    .await
            }
        }
        .map_err(|source| {
            ProductListingDetailsReadError::ProductListingDetailsQueryFailed {
                source: box_error(source),
            }
        })?;

        row.map(PersonalizedProductListingDetailsReadModel::try_from)
            .transpose()
            .map_err(|source| {
                ProductListingDetailsReadError::ProductListingDetailsReadModelInvalid {
                    source: box_error(source),
                }
            })
    }
}

pub(super) const DEFAULT_NOTIFICATION_STATES: &str = r#"
    notification_states AS (
        SELECT
            notification.product_listing_id,
            array_agg(
                notification.notification_id
                ORDER BY notification.created DESC, notification.notification_id DESC
            ) AS unseen_notification_ids
        FROM notifications notification
        WHERE notification.user_id = $2
            AND notification.seen = false
        GROUP BY notification.product_listing_id
    )
"#;

pub(super) fn product_details_select(notification_states: &str) -> String {
    SELECT_PRODUCT_DETAILS.replace("/* NOTIFICATION_STATES */", notification_states)
}

pub(super) const SELECT_PRODUCT_DETAILS: &str = r#"
    WITH /* NOTIFICATION_STATES */
    SELECT
        p.product_listing_id, p.product_listing_title_slug_id, p.current_event_id,
        p.listing_source_id, p.source_listing_id,
        listing_source.name AS listing_source_name,
        listing_source.listing_source_slug_id,
        listing_source.referral_configuration AS listing_source_referral_configuration,
        listing_source_operator.name AS listing_source_operator_name,
        listing_source_operator_partnership.partnership_id IS NOT NULL
            AS listing_source_operator_partner,
        p.title_text AS product_title_text, p.title_language AS product_title_language,
        selected_text.title_text, selected_text.title_language,
        p.price_kind, p.price_amount, p.price_currency, p.price_estimate_min_amount,
        p.price_estimate_min_currency, p.price_estimate_max_amount,
        p.price_estimate_max_currency, p.sale_observation_fx_rate_id, p.sale_observed_at, p.availability, p.lifecycle, p.url,
        p.product_images,
        assessment.decision AS content_policy_decision,
        assessment.category AS content_policy_category,
        p.auction_id, p.lot_number, p.catalogue_position, p.lot_bidding_opens_at,
        p.lot_scheduled_closes_at, p.lot_reported_closed_at,
        a.auction_id AS parent_auction_id,
        a.listing_source_id AS parent_listing_source_id,
        a.name_text AS auction_name_text,
        a.name_language AS auction_name_language,
        a.format AS auction_format,
        a.bidding_opens_at AS auction_bidding_opens_at,
        a.live_starts_at AS auction_live_starts_at,
        a.lots_begin_closing_at AS auction_lots_begin_closing_at,
        a.scheduled_end_at AS auction_scheduled_end_at,
        a.reported_status AS auction_reported_status,
        p.created, p.updated,
        $2::uuid AS personalization_user_id,
        authenticated_user.show_unassessed_or_sensitive_content AS user_show_unassessed_or_sensitive_content,
        authenticated_user.tier AS user_tier,
        watchlist.notifications AS watchlist_notifications,
        selected_match.user_search_filter_id AS selected_match_user_search_filter_id,
        selected_match.user_search_filter_name AS selected_match_user_search_filter_name,
        selected_match.feedback AS selected_match_feedback,
        selected_match.month_position AS selected_match_month_position,
        notification_state.unseen_notification_ids
    FROM product_listings p
    JOIN listing_sources listing_source
        ON listing_source.listing_source_id = p.listing_source_id
    JOIN parties listing_source_operator
        ON listing_source_operator.party_id = listing_source.operator_party_id
    LEFT JOIN partnerships listing_source_operator_partnership
        ON listing_source_operator_partnership.party_id = listing_source.operator_party_id
        AND listing_source_operator_partnership.business_state = 'ACTIVE'
    LEFT JOIN auctions a
        ON a.auction_id = p.auction_id

    LEFT JOIN product_listing_content_assessments assessment
        ON assessment.product_listing_id = p.product_listing_id
        AND assessment.source_event_id = p.content_source_event_id
    LEFT JOIN users authenticated_user ON authenticated_user.user_id = $2
    LEFT JOIN product_listing_watchlist watchlist
        ON watchlist.user_id = $2
        AND watchlist.product_listing_id = p.product_listing_id
    LEFT JOIN LATERAL (
        SELECT
            matched.user_search_filter_id,
            matched.user_search_filter_name,
            matched.feedback,
            CASE
                WHEN authenticated_user.tier = 'FREE' THEN (
                    SELECT COUNT(*)
                    FROM search_filter_matches monthly_match
                    WHERE monthly_match.user_id = $2
                        AND monthly_match.created >= (
                            date_trunc('month', matched.created AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                        )
                        AND (
                            monthly_match.created < matched.created
                            OR (
                                monthly_match.created = matched.created
                                AND (
                                    monthly_match.user_search_filter_id < matched.user_search_filter_id
                                    OR (
                                        monthly_match.user_search_filter_id = matched.user_search_filter_id
                                        AND monthly_match.product_listing_id <= matched.product_listing_id
                                    )
                                )
                            )
                        )
                )
                ELSE NULL
            END AS month_position
        FROM search_filter_matches matched
        WHERE matched.user_id = $2
            AND matched.product_listing_id = p.product_listing_id
        ORDER BY matched.created ASC, matched.user_search_filter_id ASC
        LIMIT 1
    ) AS selected_match ON TRUE
    LEFT JOIN notification_states notification_state
        ON notification_state.product_listing_id = p.product_listing_id
    LEFT JOIN LATERAL (
        SELECT
            (
                array_agg(
                    candidates.title_text
                    ORDER BY
                        CASE lower(candidates.title_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.title_language),
                        candidates.source_priority,
                        candidates.title_language,
                        candidates.title_text
                ) FILTER (WHERE candidates.title_text IS NOT NULL AND candidates.title_language IS NOT NULL)
            )[1] AS title_text,
            (
                array_agg(
                    candidates.title_language
                    ORDER BY
                        CASE lower(candidates.title_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.title_language),
                        candidates.source_priority,
                        candidates.title_language,
                        candidates.title_text
                ) FILTER (WHERE candidates.title_text IS NOT NULL AND candidates.title_language IS NOT NULL)
            )[1] AS title_language
        FROM (
            SELECT
                translation.title AS title_text,
                translation.language AS title_language,
                0 AS source_priority
            FROM product_listing_translations translation
            WHERE translation.product_listing_id = p.product_listing_id

            UNION ALL

            SELECT
                p.title_text,
                p.title_language,
                1 AS source_priority
        ) AS candidates
    ) AS selected_text ON TRUE
"#;

impl TryFrom<ProductListingDetailsRow> for PersonalizedProductListingDetailsReadModel {
    type Error = ProductListingDetailsRowMappingError;

    fn try_from(row: ProductListingDetailsRow) -> Result<Self, Self::Error> {
        let source_listing_id = SourceListingId::try_from(row.source_listing_id.clone())
            .map_err(InvalidListingRowValue::of("source listing ID"))?;
        let parsed_images = images(row.product_images.clone())?;
        let user_state = user_state(&row, &parsed_images)?;
        let content_policy = content_policy(
            row.content_policy_decision.as_deref(),
            row.content_policy_category.as_deref(),
        )?;
        let listing_auction = auction_from_parts(ProductListingAuctionParts {
            auction_id: row.auction_id,
            lot_number: row.lot_number,
            catalogue_position: row.catalogue_position,
            lot_bidding_opens_at: row.lot_bidding_opens_at,
            lot_scheduled_closes_at: row.lot_scheduled_closes_at,
            lot_reported_closed_at: row.lot_reported_closed_at,
        })
        .map_err(InvalidListingRowValue::of("lot facts"))?;
        let (auction, lot) = map_joined_auction(
            listing_auction,
            row.listing_source_id,
            JoinedAuctionParts {
                auction_id: row.parent_auction_id,
                listing_source_id: row.parent_listing_source_id,
                name_text: row.auction_name_text,
                name_language: row.auction_name_language,
                format: row.auction_format,
                bidding_opens_at: row.auction_bidding_opens_at,
                live_starts_at: row.auction_live_starts_at,
                lots_begin_closing_at: row.auction_lots_begin_closing_at,
                scheduled_end_at: row.auction_scheduled_end_at,
                reported_status: row.auction_reported_status,
            },
        )
        .map_err(InvalidListingRowValue::of("parent auction"))?;
        let title = ListingText::select(
            localized_title(row.product_title_text, row.product_title_language)?,
            localized_title(row.title_text, row.title_language)?,
        );
        let product_price =
            product_listing_price(row.price_kind, row.price_amount, row.price_currency)?;
        let product_price_estimate_min = price(
            "estimate minimum",
            row.price_estimate_min_amount,
            row.price_estimate_min_currency,
        )?;
        let product_price_estimate_max = price(
            "estimate maximum",
            row.price_estimate_max_amount,
            row.price_estimate_max_currency,
        )?;
        let sale_observation =
            sale_observation(row.sale_observed_at, row.sale_observation_fx_rate_id)?;
        let url = Url::parse(&row.url).map_err(InvalidListingRowValue::of("URL"))?;
        let view_link = view_link(
            referral_configuration(row.listing_source_referral_configuration.as_ref())
                .map_err(|()| {
                    InvalidListingRowValue::rejected(
                        "ListingSource referral configuration",
                        "unrecognized referral configuration",
                    )
                })?
                .as_ref(),
            &url,
        )
        .map_err(InvalidListingRowValue::of("view link"))?;

        Ok(Personalized {
            item: ProductListingDetailsReadModel {
                product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                    .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
                product_listing_title_slug_id: ProductListingSlugId::raw(
                    &row.product_listing_title_slug_id,
                )
                .map_err(InvalidListingRowValue::of("title slug"))?,
                event_id: try_from_uuid(row.current_event_id, "current event ID")
                    .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
                source: ListingSourceSummary {
                    listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
                        .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
                    name: ListingSourceName::try_from(row.listing_source_name)
                        .map_err(InvalidListingRowValue::of("ListingSource name"))?,
                    slug_id: ListingSourceSlugId::raw(&row.listing_source_slug_id)
                        .map_err(InvalidListingRowValue::of("ListingSource slug"))?,
                    operator: listing_source_operator(
                        row.listing_source_operator_name,
                        row.listing_source_operator_partner,
                    )
                    .map_err(InvalidListingRowValue::of("ListingSource operator name"))?,
                },
                source_listing_id,
                title,
                pricing: ProductListingPricing {
                    price: product_price,
                    price_estimate_min: product_price_estimate_min,
                    price_estimate_max: product_price_estimate_max,
                },
                sale_observation,
                availability: availability(row.availability.as_deref())?,
                lifecycle: lifecycle(&row.lifecycle)?,
                view_link,
                url,
                images: parsed_images,
                content_policy,
                auction,
                lot,
                created: row.created,
                updated: row.updated,
            },
            user_state,
        })
    }
}

fn sale_observation(
    observed_at: Option<OffsetDateTime>,
    fx_rate_id: Option<uuid::Uuid>,
) -> Result<Option<ListingSaleObservation>, ProductListingDetailsRowMappingError> {
    match (observed_at, fx_rate_id) {
        (Some(observed_at), Some(fx_rate_id)) => Ok(Some(ListingSaleObservation::new(
            observed_at,
            try_from_uuid(fx_rate_id, "sale observation FX rate ID")
                .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(InvalidListingRowValue::rejected(
            "sale observation",
            "observation time and FX rate must be present or absent together",
        )
        .into()),
    }
}

fn user_state(
    row: &ProductListingDetailsRow,
    _images: &IndexSet<ProductListingImage>,
) -> Result<Option<ProductListingUserState>, ProductListingDetailsRowMappingError> {
    let Some(personalization_user_id) = row.personalization_user_id else {
        return Ok(None);
    };
    try_from_uuid::<user_core::user_id::UserId>(personalization_user_id, "personalization user ID")
        .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?;

    let (stored_consent, tier) = match (
        row.user_show_unassessed_or_sensitive_content,
        row.user_tier.as_deref(),
    ) {
        (Some(consent), Some("FREE")) => (consent, "FREE"),
        (Some(consent), Some("PRO")) => (consent, "PRO"),
        (Some(consent), Some("ULTIMATE")) => (consent, "ULTIMATE"),
        _ => {
            return Err(InvalidListingRowValue::rejected(
                "personalization user",
                "unknown tier or missing content preference",
            )
            .into());
        }
    };
    let search_filter = search_filter_user_state(row, Some(tier))?;

    Ok(Some(ProductListingUserState {
        shipping: None,
        watchlist: WatchlistUserState {
            watching: row.watchlist_notifications.is_some(),
            notifications: row.watchlist_notifications.unwrap_or(false),
        },
        content_visibility: ContentVisibilityUserState {
            show_unassessed_or_sensitive_content: stored_consent,
        },
        notification: NotificationUserState {
            unseen_notification_ids: row
                .unseen_notification_ids
                .as_deref()
                .unwrap_or_default()
                .iter()
                .copied()
                .map(|id| {
                    try_from_uuid(id, "notification ID")
                        .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)
                })
                .collect::<Result<_, _>>()?,
        },
        search_filter,
    }))
}

fn search_filter_user_state(
    row: &ProductListingDetailsRow,
    tier: Option<&str>,
) -> Result<SearchFilterUserState, ProductListingDetailsRowMappingError> {
    let Some(user_search_filter_id) = row.selected_match_user_search_filter_id else {
        if row.selected_match_user_search_filter_name.is_some()
            || row.selected_match_feedback.is_some()
            || row.selected_match_month_position.is_some()
        {
            return Err(InvalidListingRowValue::rejected(
                "saved-search match",
                "match details without a matching search filter",
            )
            .into());
        }
        return Ok(SearchFilterUserState::default());
    };

    let hidden = match tier {
        Some("FREE") => {
            row.selected_match_month_position.ok_or_else(|| {
                InvalidListingRowValue::rejected(
                    "saved-search match",
                    "free-tier match without a monthly position",
                )
            })? > 10
        }
        Some("PRO" | "ULTIMATE") => {
            if row.selected_match_month_position.is_some() {
                return Err(InvalidListingRowValue::rejected(
                    "saved-search match",
                    "paid-tier match with a monthly position",
                )
                .into());
            }
            false
        }
        _ => {
            return Err(InvalidListingRowValue::rejected(
                "saved-search match",
                "unknown user tier",
            )
            .into());
        }
    };

    Ok(SearchFilterUserState {
        matched: true,
        hidden,
        user_search_filter_id: Some(
            try_from_uuid(user_search_filter_id, "user search filter ID")
                .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
        ),
        user_search_filter_name: row
            .selected_match_user_search_filter_name
            .clone()
            .map(UserSearchFilterName::from),
        match_feedback: row.selected_match_feedback,
        shipping_match_valuation: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxrate_core::FxRateId;

    #[test]
    fn should_map_complete_sale_observation() {
        let fx_rate_id = FxRateId::new();
        let observed_at = OffsetDateTime::UNIX_EPOCH;

        let result = sale_observation(Some(observed_at), Some(fx_rate_id.into_uuid()));

        assert!(matches!(
            result,
            Ok(Some(observation))
                if observation
                    == ListingSaleObservation::new(
                        observed_at,
                        fx_rate_id,
                    )
        ));
    }

    #[test]
    fn should_reject_incomplete_sale_observation() {
        let result = sale_observation(Some(OffsetDateTime::UNIX_EPOCH), None);

        assert!(result.is_err());
    }
}
