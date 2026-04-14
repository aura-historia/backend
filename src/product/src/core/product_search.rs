use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
use common::category_key::CategoryId;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::period_key::PeriodId;
use common::price::domain::MonetaryAmount;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::year::Year;
use serde_fields::SerdeField;
use shop::core::shop_type::ShopType;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Default, SerdeField)]
pub struct ProductSearch {
    pub language: Language,
    pub currency: Currency,
    pub product_query: Option<TextQuery<1>>,
    pub category_id: AnyOfQuery<CategoryId>,
    pub period_id: AnyOfQuery<PeriodId>,
    pub shop_name_query: AnyOfQuery<ShopName>,
    pub exclude_shop_name_query: AnyOfQuery<ShopName>,
    pub seller_name_query: AnyOfQuery<ShopName>,
    pub exclude_seller_name_query: AnyOfQuery<ShopName>,
    pub shop_slug_id_query: AnyOfQuery<SlugId<0>>,
    pub exclude_shop_slug_id_query: AnyOfQuery<SlugId<0>>,
    pub seller_slug_id_query: AnyOfQuery<SlugId<0>>,
    pub exclude_seller_slug_id_query: AnyOfQuery<SlugId<0>>,
    pub shop_type_query: AnyOfQuery<ShopType>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: AnyOfQuery<ProductState>,
    pub origin_year_query: Option<RangeQuery<Year>>,
    pub authenticity_query: AnyOfQuery<Authenticity>,
    pub condition_query: AnyOfQuery<Condition>,
    pub provenance_query: AnyOfQuery<Provenance>,
    pub restoration_query: AnyOfQuery<Restoration>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
    pub auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    pub auction_end_query: Option<RangeQuery<OffsetDateTime>>,
}

impl ProductSearch {
    pub fn new(language: Language, currency: Currency) -> Self {
        Self {
            language,
            currency,
            product_query: None,
            category_id: AnyOfQuery::default(),
            period_id: AnyOfQuery::default(),
            shop_name_query: AnyOfQuery::default(),
            exclude_shop_name_query: AnyOfQuery::default(),
            seller_name_query: AnyOfQuery::default(),
            exclude_seller_name_query: AnyOfQuery::default(),
            shop_slug_id_query: AnyOfQuery::default(),
            exclude_shop_slug_id_query: AnyOfQuery::default(),
            seller_slug_id_query: AnyOfQuery::default(),
            exclude_seller_slug_id_query: AnyOfQuery::default(),
            shop_type_query: AnyOfQuery::default(),
            price_query: None,
            state_query: AnyOfQuery::default(),
            origin_year_query: None,
            authenticity_query: AnyOfQuery::default(),
            condition_query: AnyOfQuery::default(),
            provenance_query: AnyOfQuery::default(),
            restoration_query: AnyOfQuery::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
        }
    }

    pub fn with_product_query(mut self, product_query: TextQuery<1>) -> Self {
        self.product_query = Some(product_query);
        self
    }

    pub fn with_category_id(mut self, category_id: AnyOfQuery<CategoryId>) -> Self {
        self.category_id = category_id;
        self
    }

    pub fn with_period_id(mut self, period_id: AnyOfQuery<PeriodId>) -> Self {
        self.period_id = period_id;
        self
    }

    pub fn with_shop_name_query(mut self, shop_name_query: AnyOfQuery<ShopName>) -> Self {
        self.shop_name_query = shop_name_query;
        self
    }

    pub fn with_exclude_shop_name_query(
        mut self,
        exclude_shop_name_query: AnyOfQuery<ShopName>,
    ) -> Self {
        self.exclude_shop_name_query = exclude_shop_name_query;
        self
    }

    pub fn with_seller_name_query(mut self, seller_name_query: AnyOfQuery<ShopName>) -> Self {
        self.seller_name_query = seller_name_query;
        self
    }

    pub fn with_exclude_seller_name_query(
        mut self,
        exclude_seller_name_query: AnyOfQuery<ShopName>,
    ) -> Self {
        self.exclude_seller_name_query = exclude_seller_name_query;
        self
    }

    pub fn with_shop_slug_id_query(mut self, shop_slug_id_query: AnyOfQuery<SlugId<0>>) -> Self {
        self.shop_slug_id_query = shop_slug_id_query;
        self
    }

    pub fn with_exclude_shop_slug_id_query(
        mut self,
        exclude_shop_slug_id_query: AnyOfQuery<SlugId<0>>,
    ) -> Self {
        self.exclude_shop_slug_id_query = exclude_shop_slug_id_query;
        self
    }

    pub fn with_seller_slug_id_query(
        mut self,
        seller_slug_id_query: AnyOfQuery<SlugId<0>>,
    ) -> Self {
        self.seller_slug_id_query = seller_slug_id_query;
        self
    }

    pub fn with_exclude_seller_slug_id_query(
        mut self,
        exclude_seller_slug_id_query: AnyOfQuery<SlugId<0>>,
    ) -> Self {
        self.exclude_seller_slug_id_query = exclude_seller_slug_id_query;
        self
    }

    pub fn with_shop_type_query(mut self, shop_type_query: AnyOfQuery<ShopType>) -> Self {
        self.shop_type_query = shop_type_query;
        self
    }

    pub fn with_price_query(mut self, price_query: RangeQuery<MonetaryAmount>) -> Self {
        self.price_query = Some(price_query);
        self
    }

    pub fn with_state_query(mut self, state_query: AnyOfQuery<ProductState>) -> Self {
        self.state_query = state_query;
        self
    }

    pub fn with_origin_year_query(mut self, origin_year_query: RangeQuery<Year>) -> Self {
        self.origin_year_query = Some(origin_year_query);
        self
    }

    pub fn with_authenticity_query(mut self, authenticity_query: AnyOfQuery<Authenticity>) -> Self {
        self.authenticity_query = authenticity_query;
        self
    }

    pub fn with_condition_query(mut self, condition_query: AnyOfQuery<Condition>) -> Self {
        self.condition_query = condition_query;
        self
    }

    pub fn with_provenance_query(mut self, provenance_query: AnyOfQuery<Provenance>) -> Self {
        self.provenance_query = provenance_query;
        self
    }

    pub fn with_restoration_query(mut self, restoration_query: AnyOfQuery<Restoration>) -> Self {
        self.restoration_query = restoration_query;
        self
    }

    pub fn with_created_query(mut self, created_query: RangeQuery<OffsetDateTime>) -> Self {
        self.created_query = Some(created_query);
        self
    }

    pub fn with_updated_query(mut self, updated_query: RangeQuery<OffsetDateTime>) -> Self {
        self.updated_query = Some(updated_query);
        self
    }

    pub fn with_auction_start_query(
        mut self,
        auction_start_query: RangeQuery<OffsetDateTime>,
    ) -> Self {
        self.auction_start_query = Some(auction_start_query);
        self
    }

    pub fn with_auction_end_query(mut self, auction_end_query: RangeQuery<OffsetDateTime>) -> Self {
        self.auction_end_query = Some(auction_end_query);
        self
    }
}

#[cfg(feature = "test-data")]
pub mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductSearch {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductSearch {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
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
                origin_year_query: config.fake_with_rng(rng),
                authenticity_query: config.fake_with_rng(rng),
                condition_query: config.fake_with_rng(rng),
                provenance_query: config.fake_with_rng(rng),
                restoration_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                auction_start_query: fake_range_query_datetime(config, rng),
                auction_end_query: fake_range_query_datetime(config, rng),
            }
        }
    }

    pub fn fake_range_query_datetime<R: fake::RngExt + ?Sized>(
        config: &Faker,
        rng: &mut R,
    ) -> Option<RangeQuery<OffsetDateTime>> {
        if config.fake_with_rng(rng) {
            None
        } else {
            let min = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            let max = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            Some(RangeQuery { min, max })
        }
    }
}
