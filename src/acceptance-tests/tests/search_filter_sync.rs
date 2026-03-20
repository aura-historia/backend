use aws_tests_common::get_cfn_output;
use common::category_key::CategoryId;
use common::currency::data::CurrencyData;
use common::language::data::LanguageData;
use common::period_key::PeriodId;
use common::query::range_query::RangeQuery;
use common::shop_name::ShopName;
use opensearch::GetParts;
use opensearch::indices::IndicesRefreshParts;
use product::data::authenticity_data::AuthenticityData;
use product::data::condition_data::ConditionData;
use product::data::product_search_data::ProductSearchData;
use product::data::product_state_data::ProductStateData;
use product::data::provenance_data::ProvenanceData;
use product::data::restoration_data::RestorationData;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::opensearch::user_search_filter_document::UserSearchFilterDocument;
use search_filter_api::{
    patch_types::{PatchProductSearchData, PatchUserSearchFilterData},
    post_types::PostUserSearchFilterData,
};
use shop::data::shop_type_data::ShopTypeData;
use std::time::Duration;
use test_api::*;
use time::macros::datetime;

async fn try_read_by_id<T: serde::de::DeserializeOwned>(
    index: &str,
    id: impl Into<String>,
) -> Option<T> {
    let get_response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id.into()))
        .send()
        .await
        .unwrap();

    if get_response.status_code().as_u16() == 404 {
        return None;
    }

    let get_response = get_response.error_for_status_code().unwrap();
    let response_doc: serde_json::Value = get_response.json().await.unwrap();
    Some(serde_json::from_value(response_doc["_source"].clone()).unwrap())
}

async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .indices()
        .refresh(IndicesRefreshParts::Index(&[index]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
}

async fn wait_until_document_exists(
    user_search_filter_id: impl Into<String>,
) -> UserSearchFilterDocument {
    let user_search_filter_id = user_search_filter_id.into();

    for _ in 0..24 {
        refresh_index("user_search_filters").await;

        if let Some(document) = try_read_by_id::<UserSearchFilterDocument>(
            "user_search_filters",
            &user_search_filter_id,
        )
        .await
        {
            return document;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    panic!(
        "Expected search-filter document '{}' to exist in OpenSearch, but it did not appear in time.",
        user_search_filter_id
    );
}

async fn wait_until_document_deleted(user_search_filter_id: impl Into<String>) {
    let user_search_filter_id = user_search_filter_id.into();

    for _ in 0..24 {
        refresh_index("user_search_filters").await;

        if try_read_by_id::<UserSearchFilterDocument>("user_search_filters", &user_search_filter_id)
            .await
            .is_none()
        {
            return;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    panic!(
        "Expected search-filter document '{}' to be deleted from OpenSearch, but it still existed.",
        user_search_filter_id
    );
}

#[localstack_test(services = [Cloudformation()])]
async fn should_create_search_filter_and_sync_it_to_opensearch() {
    let user = create_random_test_user().await;

    let search_data = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Barock Kommode".try_into().unwrap()),
        category_id: HashSet::from_iter([CategoryId::from("furniture")]),
        period_id: HashSet::from_iter([PeriodId::from("baroque")]),
        shop_name_query: HashSet::from_iter([ShopName::from("Galerie Test")]),
        exclude_shop_name_query: HashSet::from_iter([ShopName::from("Do Not Match Shop")]),
        shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
        price_query: Some(RangeQuery {
            min: Some(100),
            max: Some(5000),
        }),
        state_query: HashSet::from_iter([ProductStateData::Available]),
        origin_year_query: Some(RangeQuery {
            min: Some(1700.into()),
            max: Some(1800.into()),
        }),
        authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
        condition_query: HashSet::from_iter([ConditionData::Excellent]),
        provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
        restoration_query: HashSet::from_iter([RestorationData::Minor]),
        created_query: Some(RangeQuery {
            min: Some(datetime!(2020-01-01 0:00 UTC)),
            max: Some(datetime!(2030-01-01 0:00 UTC)),
        }),
        updated_query: Some(RangeQuery {
            min: Some(datetime!(2020-01-01 0:00 UTC)),
            max: Some(datetime!(2030-01-01 0:00 UTC)),
        }),
        auction_start_query: Some(RangeQuery {
            min: Some(datetime!(2024-01-01 0:00 UTC)),
            max: Some(datetime!(2026-01-01 0:00 UTC)),
        }),
        auction_end_query: Some(RangeQuery {
            min: Some(datetime!(2024-01-01 0:00 UTC)),
            max: Some(datetime!(2026-01-01 0:00 UTC)),
        }),
    };

    let expected = PostUserSearchFilterData {
        name: "Staging sync create".into(),
        search: search_data,
    };

    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&expected)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(expected.name, posted.name);
    assert_eq!(expected.search, posted.search);
    assert_eq!(user.sub.to_string(), posted.user_id.to_string());

    let document = wait_until_document_exists(posted.user_search_filter_id.to_string()).await;

    assert_eq!(posted.user_search_filter_id, document.user_search_filter_id);
    assert_eq!(posted.user_id, document.user_id);
    assert_eq!(posted.name, document.name);
    assert_eq!(posted.created, document.created);
    assert_eq!(posted.updated, document.updated);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_update_search_filter_and_sync_changes_to_opensearch() {
    let user = create_random_test_user().await;

    let initial_search = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Barock".try_into().unwrap()),
        category_id: HashSet::from_iter([CategoryId::from("furniture")]),
        period_id: HashSet::from_iter([PeriodId::from("baroque")]),
        shop_name_query: HashSet::from_iter([ShopName::from("Initial Shop")]),
        exclude_shop_name_query: HashSet::from_iter([ShopName::from("Initial Excluded Shop")]),
        shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
        price_query: Some(RangeQuery {
            min: Some(50),
            max: Some(1000),
        }),
        state_query: HashSet::from_iter([ProductStateData::Available]),
        origin_year_query: Some(RangeQuery {
            min: Some(1680.into()),
            max: Some(1780.into()),
        }),
        authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
        condition_query: HashSet::from_iter([ConditionData::Good]),
        provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
        restoration_query: HashSet::from_iter([RestorationData::None]),
        created_query: Some(RangeQuery {
            min: Some(datetime!(2021-01-01 0:00 UTC)),
            max: Some(datetime!(2031-01-01 0:00 UTC)),
        }),
        updated_query: Some(RangeQuery {
            min: Some(datetime!(2021-01-01 0:00 UTC)),
            max: Some(datetime!(2031-01-01 0:00 UTC)),
        }),
        auction_start_query: Some(RangeQuery {
            min: Some(datetime!(2024-01-01 0:00 UTC)),
            max: Some(datetime!(2025-01-01 0:00 UTC)),
        }),
        auction_end_query: Some(RangeQuery {
            min: Some(datetime!(2024-01-01 0:00 UTC)),
            max: Some(datetime!(2025-01-01 0:00 UTC)),
        }),
    };

    let initial = PostUserSearchFilterData {
        name: "Staging sync update initial".into(),
        search: initial_search,
    };

    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&initial)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();
    let initial_document =
        wait_until_document_exists(posted.user_search_filter_id.to_string()).await;
    assert_eq!(posted.name, initial_document.name);

    let patch = PatchUserSearchFilterData {
        name: Some("Staging sync update patched".into()),
        notifications: None,
        search: Some(PatchProductSearchData {
            language: Some(LanguageData::Fr),
            currency: Some(CurrencyData::Usd),
            product_query: Some("Louis XV".try_into().unwrap()),
            category_id: Some(HashSet::from_iter([CategoryId::from("decorative-objects")])),
            period_id: Some(HashSet::from_iter([PeriodId::from("rococo")])),
            shop_name_query: Some(HashSet::from_iter([ShopName::from("Patched Shop")])),
            shop_type_query: Some(HashSet::from_iter([ShopTypeData::AuctionHouse])),
            price_query: Some(RangeQuery {
                min: Some(500),
                max: Some(25_000),
            }),
            state_query: Some(HashSet::from_iter([ProductStateData::Sold])),
            origin_year_query: Some(RangeQuery {
                min: Some(1720.into()),
                max: Some(1790.into()),
            }),
            authenticity_query: Some(HashSet::from_iter([AuthenticityData::LaterCopy])),
            condition_query: Some(HashSet::from_iter([ConditionData::Fair])),
            provenance_query: Some(HashSet::from_iter([ProvenanceData::Claimed])),
            restoration_query: Some(HashSet::from_iter([RestorationData::Major])),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2022-01-01 0:00 UTC)),
                max: Some(datetime!(2032-01-01 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2023-01-01 0:00 UTC)),
                max: Some(datetime!(2033-01-01 0:00 UTC)),
            }),
        }),
    };

    let patch_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.user_search_filter_id
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .json(&patch)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());

    let patched = patch_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(patch.name.as_ref().unwrap(), &patched.name);
    assert_eq!(
        patch.search.as_ref().unwrap().language.as_ref().unwrap(),
        &patched.search.language
    );
    assert_eq!(
        patch.search.as_ref().unwrap().currency.as_ref().unwrap(),
        &patched.search.currency
    );
    assert_eq!(
        patch
            .search
            .as_ref()
            .unwrap()
            .product_query
            .as_ref()
            .unwrap(),
        patched.search.product_query.as_ref().unwrap()
    );

    tokio::time::sleep(Duration::from_secs(30)).await;
    let patched_document =
        wait_until_document_exists(patched.user_search_filter_id.to_string()).await;

    assert_eq!(
        patched.user_search_filter_id,
        patched_document.user_search_filter_id
    );
    assert_eq!(patched.user_id, patched_document.user_id);
    assert_eq!(patched.name, patched_document.name);
    assert_eq!(patched.created, patched_document.created);
    assert_eq!(patched.updated, patched_document.updated);
    assert!(patched.updated >= posted.updated);
    assert_ne!(initial_document.query, patched_document.query);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_delete_search_filter_and_remove_it_from_opensearch() {
    let user = create_random_test_user().await;

    let search_data = ProductSearchData {
        language: LanguageData::En,
        currency: CurrencyData::Gbp,
        product_query: Some("Georgian cabinet".try_into().unwrap()),
        category_id: HashSet::from_iter([CategoryId::from("furniture")]),
        period_id: HashSet::from_iter([PeriodId::from("georgian")]),
        shop_name_query: HashSet::from_iter([ShopName::from("Delete Me Shop")]),
        exclude_shop_name_query: HashSet::from_iter([ShopName::from("Excluded Delete Shop")]),
        shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
        price_query: Some(RangeQuery {
            min: Some(200),
            max: Some(12000),
        }),
        state_query: HashSet::from_iter([ProductStateData::Available]),
        origin_year_query: Some(RangeQuery {
            min: Some(1714.into()),
            max: Some(1830.into()),
        }),
        authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
        condition_query: HashSet::from_iter([ConditionData::Great]),
        provenance_query: HashSet::from_iter([ProvenanceData::Complete]),
        restoration_query: HashSet::from_iter([RestorationData::Minor]),
        created_query: Some(RangeQuery {
            min: Some(datetime!(2020-01-01 0:00 UTC)),
            max: Some(datetime!(2030-01-01 0:00 UTC)),
        }),
        updated_query: Some(RangeQuery {
            min: Some(datetime!(2020-01-01 0:00 UTC)),
            max: Some(datetime!(2030-01-01 0:00 UTC)),
        }),
        auction_start_query: Some(RangeQuery {
            min: Some(datetime!(2024-01-01 0:00 UTC)),
            max: Some(datetime!(2026-01-01 0:00 UTC)),
        }),
        auction_end_query: Some(RangeQuery {
            min: Some(datetime!(2024-01-01 0:00 UTC)),
            max: Some(datetime!(2026-01-01 0:00 UTC)),
        }),
    };

    let expected = PostUserSearchFilterData {
        name: "Staging sync delete".into(),
        search: search_data,
    };

    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&expected)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();

    let document = wait_until_document_exists(posted.user_search_filter_id.to_string()).await;
    assert_eq!(posted.user_search_filter_id, document.user_search_filter_id);

    let delete_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.user_search_filter_id
    );
    let delete_response = reqwest::Client::new()
        .delete(delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());

    wait_until_document_deleted(posted.user_search_filter_id.to_string()).await;
}
