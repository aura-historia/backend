use common::pagination::cursor::api::JsonCursoredData;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use test_api::*;
use user::core::role::UserRole;
use user::data::get_user_data::GetUserAccountData;
use user::dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};
use user::dynamodb::user_record::UserRecord;
use user::opensearch::repository::{UserOpenSearchRepository, UserOpenSearchRepositoryImpl};
use user::opensearch::user_document::UserDocument;
use user::service::cognito_admin_service::MockCognitoAdminService;
use user::service::user_service::UserServiceImpl;
use user_api::handler;

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_200_filter_users_when_geo_filters_are_given_for_admin_search() {
    let dynamodb_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let opensearch_repository = UserOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let cognito_admin_service = MockCognitoAdminService::default();
    let service = UserServiceImpl::with_cognito_and_opensearch(
        &dynamodb_repository,
        &cognito_admin_service,
        &opensearch_repository,
    );
    let mut admin = Faker.fake::<user::core::user::User>();
    admin.role = UserRole::Admin;
    dynamodb_repository
        .put_user_record(UserRecord::from(admin.clone()))
        .await
        .unwrap();
    let mut expected = Faker.fake::<UserDocument>();
    expected.structured_address_country = Some(isocountry::CountryCode::DEU);
    expected.structured_address_continent = Some(geo::data::continent_data::ContinentData::Europe);
    expected.geo_address = Some("52.5200,13.4050".to_string());
    let mut other = Faker.fake::<UserDocument>();
    other.structured_address_country = Some(isocountry::CountryCode::USA);
    other.structured_address_continent =
        Some(geo::data::continent_data::ContinentData::NorthAmerica);
    other.geo_address = Some("40.7128,-74.0060".to_string());
    opensearch_repository
        .index_user_document(expected.clone())
        .await
        .unwrap();
    opensearch_repository
        .index_user_document(other)
        .await
        .unwrap();
    refresh_index("users").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/users")
            .raw_query_string(
                "country=DE&continent=EUROPE&geoAddress[lat]=52.52&geoAddress[lon]=13.405&geoAddress[distance][amount]=50&geoAddress[distance][unit]=KILOMETERS"
                    .to_string(),
            )
            .jwt_claim("sub", admin.user_id)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let actual: JsonCursoredData<GetUserAccountData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(Some(1), actual.total);
    assert_eq!(expected.user_id, actual.items[0].user_id);
}
