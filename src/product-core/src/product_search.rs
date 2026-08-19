use common::distance::domain::GeoDistanceQuery;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::string_newtype;
use geo::core::continent::Continent;
use isocountry::CountryCode;
use localization::Language;
use money::Currency;
use money::MonetaryAmount;
use serde_fields::SerdeField;
use shop_core::shop_type::ShopType;
use time::OffsetDateTime;

string_newtype!(EnhancedSearchDescription, max_length(1000));

#[derive(Debug, Clone, PartialEq, Default, SerdeField)]
pub struct ProductSearch {
    pub language: Language,
    pub currency: Currency,
    pub product_query: Vec<TextQuery<1>>,
    pub enhanced_search_description: Option<EnhancedSearchDescription>,
    pub exclude_product_id_query: AnyOfQuery<ProductId>,
    pub shop_name_query: AnyOfQuery<ShopName>,
    pub exclude_shop_name_query: AnyOfQuery<ShopName>,
    pub seller_name_query: AnyOfQuery<ShopName>,
    pub exclude_seller_name_query: AnyOfQuery<ShopName>,
    pub shop_slug_id_query: AnyOfQuery<ShopSlugId>,
    pub exclude_shop_slug_id_query: AnyOfQuery<ShopSlugId>,
    pub seller_slug_id_query: AnyOfQuery<SellerSlugId>,
    pub exclude_seller_slug_id_query: AnyOfQuery<SellerSlugId>,
    pub shop_type_query: AnyOfQuery<ShopType>,
    pub country_query: AnyOfQuery<CountryCode>,
    pub continent_query: AnyOfQuery<Continent>,
    pub geo_address_distance_query: Option<GeoDistanceQuery>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: AnyOfQuery<ProductState>,
    pub lifecycle_query: AnyOfQuery<ProductLifecycle>,
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
            product_query: Vec::new(),
            enhanced_search_description: None,
            exclude_product_id_query: AnyOfQuery::default(),
            shop_name_query: AnyOfQuery::default(),
            exclude_shop_name_query: AnyOfQuery::default(),
            seller_name_query: AnyOfQuery::default(),
            exclude_seller_name_query: AnyOfQuery::default(),
            shop_slug_id_query: AnyOfQuery::default(),
            exclude_shop_slug_id_query: AnyOfQuery::default(),
            seller_slug_id_query: AnyOfQuery::default(),
            exclude_seller_slug_id_query: AnyOfQuery::default(),
            shop_type_query: AnyOfQuery::default(),
            country_query: AnyOfQuery::default(),
            continent_query: AnyOfQuery::default(),
            geo_address_distance_query: None,
            price_query: None,
            state_query: AnyOfQuery::default(),
            lifecycle_query: AnyOfQuery::default(),
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
        }
    }

    pub fn with_product_query(mut self, product_query: TextQuery<1>) -> Self {
        self.product_query.push(product_query);
        self
    }

    pub fn with_enhanced_search_description(
        mut self,
        enhanced_search_description: EnhancedSearchDescription,
    ) -> Self {
        self.enhanced_search_description = Some(enhanced_search_description);
        self
    }

    pub fn with_exclude_product_id_query(
        mut self,
        exclude_product_id_query: AnyOfQuery<ProductId>,
    ) -> Self {
        self.exclude_product_id_query = exclude_product_id_query;
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

    pub fn with_shop_slug_id_query(mut self, shop_slug_id_query: AnyOfQuery<ShopSlugId>) -> Self {
        self.shop_slug_id_query = shop_slug_id_query;
        self
    }

    pub fn with_exclude_shop_slug_id_query(
        mut self,
        exclude_shop_slug_id_query: AnyOfQuery<ShopSlugId>,
    ) -> Self {
        self.exclude_shop_slug_id_query = exclude_shop_slug_id_query;
        self
    }

    pub fn with_seller_slug_id_query(
        mut self,
        seller_slug_id_query: AnyOfQuery<SellerSlugId>,
    ) -> Self {
        self.seller_slug_id_query = seller_slug_id_query;
        self
    }

    pub fn with_exclude_seller_slug_id_query(
        mut self,
        exclude_seller_slug_id_query: AnyOfQuery<SellerSlugId>,
    ) -> Self {
        self.exclude_seller_slug_id_query = exclude_seller_slug_id_query;
        self
    }

    pub fn with_shop_type_query(mut self, shop_type_query: AnyOfQuery<ShopType>) -> Self {
        self.shop_type_query = shop_type_query;
        self
    }

    pub fn with_country_query(mut self, country_query: AnyOfQuery<CountryCode>) -> Self {
        self.country_query = country_query;
        self
    }

    pub fn with_continent_query(mut self, continent_query: AnyOfQuery<Continent>) -> Self {
        self.continent_query = continent_query;
        self
    }

    pub fn with_geo_address_distance_query(
        mut self,
        geo_address_distance_query: GeoDistanceQuery,
    ) -> Self {
        self.geo_address_distance_query = Some(geo_address_distance_query);
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

    pub fn with_lifecycle_query(mut self, lifecycle_query: AnyOfQuery<ProductLifecycle>) -> Self {
        self.lifecycle_query = lifecycle_query;
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

#[cfg(test)]
mod tests {
    use super::*;
    use common::query::range_query::RangeQuery;
    use std::collections::HashSet;

    #[test]
    fn should_create_product_search_with_language_and_currency() {
        let search = ProductSearch::new(Language::En, Currency::Eur);

        assert_eq!(Language::En, search.language);
        assert_eq!(Currency::Eur, search.currency);
        assert!(search.product_query.is_empty());
    }

    #[test]
    fn should_set_builder_fields() {
        let product_id = ProductId::new();
        let search = ProductSearch::new(Language::En, Currency::Usd)
            .with_product_query(text_query("vase"))
            .with_enhanced_search_description(EnhancedSearchDescription::from("bronze"))
            .with_exclude_product_id_query(HashSet::from([product_id]).into())
            .with_state_query(HashSet::from([ProductState::Listed]).into())
            .with_lifecycle_query(HashSet::from([ProductLifecycle::Active]).into())
            .with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(10_u64)),
                max: Some(MonetaryAmount::from(20_u64)),
            });

        assert_eq!(1, search.product_query.len());
        assert!(search.enhanced_search_description.is_some());
        assert!(search.exclude_product_id_query.contains(&product_id));
        assert!(search.state_query.contains(&ProductState::Listed));
        assert!(search.lifecycle_query.contains(&ProductLifecycle::Active));
        assert!(search.price_query.is_some());
    }

    #[test]
    fn should_truncate_enhanced_search_description() {
        let description = EnhancedSearchDescription::from("a".repeat(1200));

        assert_eq!(1000, description.as_ref().len());
    }

    fn text_query(value: &str) -> TextQuery<1> {
        match TextQuery::try_from(value) {
            Ok(query) => query,
            Err(error) => panic!("invalid test text query: {error}"),
        }
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
                enhanced_search_description: None,
                exclude_product_id_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                exclude_shop_name_query: config.fake_with_rng(rng),
                seller_name_query: config.fake_with_rng(rng),
                exclude_seller_name_query: config.fake_with_rng(rng),
                shop_slug_id_query: config.fake_with_rng(rng),
                exclude_shop_slug_id_query: config.fake_with_rng(rng),
                seller_slug_id_query: config.fake_with_rng(rng),
                exclude_seller_slug_id_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                country_query: Default::default(),
                continent_query: config.fake_with_rng(rng),
                geo_address_distance_query: None,
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                lifecycle_query: Default::default(),
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_search_without_lifecycle_query() {
            let search = Faker.fake::<ProductSearch>();

            assert!(search.lifecycle_query.is_empty());
        }
    }
}
