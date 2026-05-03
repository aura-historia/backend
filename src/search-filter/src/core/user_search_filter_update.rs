use crate::core::user_search_filter::EnhancedSearchDescription;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record_update::UserSearchFilterRecordUpdate;
use common::category_key::CategoryId;
use common::period_key::PeriodId;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::domain::ResourceState;
use common::resource_state::record::ResourceStateRecord;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::year::Year;
use common::{
    currency::{domain::Currency, record::CurrencyRecord},
    language::{domain::Language, record::LanguageRecord},
    price::domain::MonetaryAmount,
    product_state::domain::ProductState,
};
use product::core::authenticity::Authenticity;
use product::core::condition::Condition;
use product::core::provenance::Provenance;
use product::core::restoration::Restoration;
use product::dynamodb::authenticity_record::AuthenticityRecord;
use product::dynamodb::condition_record::ConditionRecord;
use product::dynamodb::product_state_record::ProductStateRecord;
use product::dynamodb::provenance_record::ProvenanceRecord;
use product::dynamodb::restoration_record::RestorationRecord;
use shop::core::shop_type::ShopType;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct UserSearchFilterUpdate {
    pub name: Option<UserSearchFilterName>,
    pub enhanced_search_description: Option<EnhancedSearchDescription>,
    pub notifications: Option<bool>,
    pub state: Option<ResourceState>,
    pub product_query: Option<TextQuery<1>>,
    pub category_id: Option<AnyOfQuery<CategoryId>>,
    pub period_id: Option<AnyOfQuery<PeriodId>>,
    pub shop_name_query: Option<HashSet<ShopName>>,
    pub exclude_shop_name_query: Option<HashSet<ShopName>>,
    pub seller_name_query: Option<HashSet<ShopName>>,
    pub exclude_seller_name_query: Option<HashSet<ShopName>>,
    pub shop_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub exclude_shop_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub seller_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub exclude_seller_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub shop_type_query: Option<AnyOfQuery<ShopType>>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: Option<AnyOfQuery<ProductState>>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
    pub origin_year_query: Option<RangeQuery<Year>>,
    pub authenticity_query: Option<AnyOfQuery<Authenticity>>,
    pub condition_query: Option<AnyOfQuery<Condition>>,
    pub provenance_query: Option<AnyOfQuery<Provenance>>,
    pub restoration_query: Option<AnyOfQuery<Restoration>>,
    pub auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    pub auction_end_query: Option<RangeQuery<OffsetDateTime>>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub updated: OffsetDateTime,
}

impl UserSearchFilterUpdate {
    pub fn is_empty(&self) -> bool {
        let UserSearchFilterUpdate {
            name,
            enhanced_search_description,
            notifications,
            state,
            product_query,
            category_id,
            period_id,
            shop_name_query,
            exclude_shop_name_query,
            seller_name_query,
            exclude_seller_name_query,
            shop_slug_id_query,
            exclude_shop_slug_id_query,
            seller_slug_id_query,
            exclude_seller_slug_id_query,
            shop_type_query,
            price_query,
            state_query,
            created_query,
            updated_query,
            origin_year_query,
            authenticity_query,
            condition_query,
            provenance_query,
            restoration_query,
            auction_start_query,
            auction_end_query,
            language,
            currency,
            updated: _,
        } = self;

        name.is_none()
            && enhanced_search_description.is_none()
            && notifications.is_none()
            && state.is_none()
            && product_query.is_none()
            && category_id.is_none()
            && period_id.is_none()
            && shop_name_query.is_none()
            && exclude_shop_name_query.is_none()
            && seller_name_query.is_none()
            && exclude_seller_name_query.is_none()
            && shop_slug_id_query.is_none()
            && exclude_shop_slug_id_query.is_none()
            && seller_slug_id_query.is_none()
            && exclude_seller_slug_id_query.is_none()
            && shop_type_query.is_none()
            && price_query.is_none()
            && state_query.is_none()
            && created_query.is_none()
            && updated_query.is_none()
            && origin_year_query.is_none()
            && authenticity_query.is_none()
            && condition_query.is_none()
            && provenance_query.is_none()
            && restoration_query.is_none()
            && auction_start_query.is_none()
            && auction_end_query.is_none()
            && language.is_none()
            && currency.is_none()
    }
}

impl From<UserSearchFilterUpdate> for UserSearchFilterRecordUpdate {
    fn from(update: UserSearchFilterUpdate) -> Self {
        UserSearchFilterRecordUpdate {
            name: update.name,
            notifications: update.notifications,
            state: update.state.map(ResourceStateRecord::from),
            product_query: update.product_query,
            category_id: update.category_id.map(HashSet::from),
            period_id: update.period_id.map(HashSet::from),
            shop_name_query: update.shop_name_query,
            exclude_shop_name_query: update.exclude_shop_name_query,
            seller_name_query: update.seller_name_query,
            exclude_seller_name_query: update.exclude_seller_name_query,
            shop_slug_id_query: update.shop_slug_id_query,
            exclude_shop_slug_id_query: update.exclude_shop_slug_id_query,
            seller_slug_id_query: update.seller_slug_id_query,
            exclude_seller_slug_id_query: update.exclude_seller_slug_id_query,
            shop_type_query: update
                .shop_type_query
                .map(|types| types.into_iter().map(ShopTypeRecord::from).collect()),
            price_query: update
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: update
                .state_query
                .map(|states| states.into_iter().map(ProductStateRecord::from).collect()),
            created_query: update.created_query,
            updated_query: update.updated_query,
            origin_year_query: update.origin_year_query,
            authenticity_query: update
                .authenticity_query
                .map(|values| values.into_iter().map(AuthenticityRecord::from).collect()),
            condition_query: update
                .condition_query
                .map(|values| values.into_iter().map(ConditionRecord::from).collect()),
            provenance_query: update
                .provenance_query
                .map(|values| values.into_iter().map(ProvenanceRecord::from).collect()),
            restoration_query: update
                .restoration_query
                .map(|values| values.into_iter().map(RestorationRecord::from).collect()),
            auction_start_query: update.auction_start_query,
            auction_end_query: update.auction_end_query,
            language: update.language.map(LanguageRecord::from),
            currency: update.currency.map(CurrencyRecord::from),
            updated: update.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user_search_filter_update::UserSearchFilterUpdate;
    use fake::{Dummy, Fake, Faker};
    use product::core::product_search::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for UserSearchFilterUpdate {
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterUpdate {
                name: config.fake_with_rng(rng),
                enhanced_search_description: config.fake_with_rng(rng),
                notifications: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                period_id: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                exclude_shop_name_query: config.fake_with_rng(rng),
                seller_name_query: config.fake_with_rng(rng),
                exclude_seller_name_query: config.fake_with_rng(rng),
                shop_slug_id_query: config.fake_with_rng(rng),
                exclude_shop_slug_id_query: config.fake_with_rng(rng),
                seller_slug_id_query: config.fake_with_rng(rng),
                exclude_seller_slug_id_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                origin_year_query: config.fake_with_rng(rng),
                authenticity_query: config.fake_with_rng(rng),
                condition_query: config.fake_with_rng(rng),
                provenance_query: config.fake_with_rng(rng),
                restoration_query: config.fake_with_rng(rng),
                auction_start_query: config.fake_with_rng(rng),
                auction_end_query: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
