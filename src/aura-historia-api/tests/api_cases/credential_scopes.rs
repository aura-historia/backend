use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{json_response, seed_access_token_for, seed_user};
use reqwest::StatusCode;
use test_api::{IntegrationTestService, aura_integration_test};
use user_core::access_token::Scope;

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_require_resource_read_scopes_for_private_collections() {
    let admin_id = seed_user("ADMIN").await;
    let user_id = seed_user("USER").await;
    let client = reqwest::Client::new();
    for (path, scope, write_scope, admin_only) in [
        (
            "/v1/admin/product-listings",
            Scope::ProductListingsRead,
            Scope::ProductListingsWrite,
            true,
        ),
        (
            "/v1/admin/auctions",
            Scope::AuctionsRead,
            Scope::AuctionsWrite,
            true,
        ),
        (
            "/v1/admin/parties",
            Scope::PartiesRead,
            Scope::PartiesWrite,
            true,
        ),
        (
            "/v1/admin/listing-sources",
            Scope::ListingSourcesRead,
            Scope::ListingSourcesWrite,
            true,
        ),
        (
            "/v1/admin/partnership-applications",
            Scope::PartnershipApplicationsRead,
            Scope::PartnershipApplicationsWrite,
            true,
        ),
        (
            "/v1/admin/partnerships",
            Scope::PartnershipsRead,
            Scope::PartnershipsWrite,
            true,
        ),
        (
            "/v1/admin/overview",
            Scope::AdminOverviewRead,
            Scope::UsersRead,
            true,
        ),
        (
            "/v1/me/notifications",
            Scope::NotificationsRead,
            Scope::NotificationsWrite,
            false,
        ),
        (
            "/v1/me/search-filters",
            Scope::SearchFiltersRead,
            Scope::SearchFiltersWrite,
            false,
        ),
        (
            "/v1/me/partnership-applications",
            Scope::PartnershipApplicationsRead,
            Scope::PartnershipApplicationsWrite,
            false,
        ),
        (
            "/v1/me/listing-sources",
            Scope::ListingSourcesRead,
            Scope::ListingSourcesWrite,
            false,
        ),
    ] {
        for (owner, scopes, expected) in [
            (
                admin_id,
                std::collections::HashSet::new(),
                StatusCode::FORBIDDEN,
            ),
            (
                admin_id,
                std::collections::HashSet::from([write_scope]),
                StatusCode::FORBIDDEN,
            ),
            (
                admin_id,
                std::collections::HashSet::from([scope]),
                StatusCode::OK,
            ),
            (
                user_id,
                std::collections::HashSet::from([scope]),
                if admin_only {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::OK
                },
            ),
        ] {
            let token = seed_access_token_for(owner, scopes).await;
            let response = client
                .get(format!("{}{path}", AURA_API.base_url()))
                .bearer_auth(String::from(token))
                .send()
                .await
                .expect("private read response");
            let (status, body) = json_response(response).await;
            assert_eq!(expected, status, "{path}: {body}");
            if expected == StatusCode::FORBIDDEN {
                assert_eq!("FORBIDDEN", body["error"], "{path}: {body}");
            }
        }
    }
}
