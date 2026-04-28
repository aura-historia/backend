use common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery};
use common::query::any_of_query::AnyOfQuery;
use common::sort::{Sort, SortOrder};
use fake::{Fake, Faker};
use geo::core::continent::Continent;
use geo::data::continent_data::ContinentData;
use std::time::Duration;
use test_api::*;
use user::core::sort_user_field::SortUserField;
use user::core::user_search::UserSearch;
use user::opensearch::repository::{UserOpenSearchRepository, UserOpenSearchRepositoryImpl};
use user::opensearch::user_document::UserDocument;

#[localstack_test(services = [OpenSearch()])]
async fn should_search_user_documents_when_country_query_is_given() {
    let repository = UserOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<UserDocument>();
    expected.structured_address_country = Some(isocountry::CountryCode::DEU);
    expected.structured_address_continent = Some(ContinentData::Europe);
    let mut other = Faker.fake::<UserDocument>();
    other.structured_address_country = Some(isocountry::CountryCode::USA);
    other.structured_address_continent = Some(ContinentData::NorthAmerica);
    repository
        .index_user_document(expected.clone())
        .await
        .unwrap();
    repository.index_user_document(other).await.unwrap();
    refresh_index("users").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let search = UserSearch {
        country_query: AnyOfQuery::from(std::collections::HashSet::from_iter([
            isocountry::CountryCode::DEU,
        ])),
        ..Default::default()
    };

    let response = repository
        .search_user_documents(
            &search,
            &Sort {
                sort: SortUserField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    let hits = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.user_id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        std::collections::HashSet::from_iter([expected.user_id]),
        hits
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_user_documents_when_continent_query_is_given() {
    let repository = UserOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<UserDocument>();
    expected.structured_address_country = Some(isocountry::CountryCode::DEU);
    expected.structured_address_continent = Some(ContinentData::Europe);
    let mut other = Faker.fake::<UserDocument>();
    other.structured_address_country = Some(isocountry::CountryCode::JPN);
    other.structured_address_continent = Some(ContinentData::Asia);
    repository
        .index_user_document(expected.clone())
        .await
        .unwrap();
    repository.index_user_document(other).await.unwrap();
    refresh_index("users").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let search = UserSearch {
        continent_query: AnyOfQuery::from(std::collections::HashSet::from_iter([
            Continent::Europe,
        ])),
        ..Default::default()
    };

    let response = repository
        .search_user_documents(
            &search,
            &Sort {
                sort: SortUserField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    let hits = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.user_id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        std::collections::HashSet::from_iter([expected.user_id]),
        hits
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_user_documents_when_geo_address_distance_query_is_given() {
    let repository = UserOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut expected = Faker.fake::<UserDocument>();
    expected.geo_address = Some("52.5200,13.4050".to_string());
    let mut other = Faker.fake::<UserDocument>();
    other.geo_address = Some("40.7128,-74.0060".to_string());
    repository
        .index_user_document(expected.clone())
        .await
        .unwrap();
    repository.index_user_document(other).await.unwrap();
    refresh_index("users").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let search = UserSearch {
        geo_address_distance_query: Some(GeoDistanceQuery {
            lat: 52.5200,
            lon: 13.4050,
            distance: Distance {
                amount: 50.0,
                unit: DistanceUnit::Kilometers,
            },
        }),
        ..Default::default()
    };

    let response = repository
        .search_user_documents(
            &search,
            &Sort {
                sort: SortUserField::Score,
                order: SortOrder::Desc,
            },
            &None,
        )
        .await
        .unwrap();

    let hits = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source.user_id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        std::collections::HashSet::from_iter([expected.user_id]),
        hits
    );
}
