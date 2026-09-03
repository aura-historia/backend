mod api_support;

use test_api::{AuraHistoriaApi, OpenSearch, Postgres};

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const OPENSEARCH: OpenSearch = OpenSearch();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[path = "api_cases/billing.rs"]
mod billing;
#[path = "api_cases/newsletter.rs"]
mod newsletter;
#[path = "api_cases/notifications.rs"]
mod notifications;
#[path = "api_cases/oauth.rs"]
mod oauth;
#[path = "api_cases/parties.rs"]
mod parties;
#[path = "api_cases/partner_product_listings.rs"]
mod partner_product_listings;
#[path = "api_cases/product_listings.rs"]
mod product_listings;
#[path = "api_cases/search_filters.rs"]
mod search_filters;
#[path = "api_cases/users.rs"]
mod users;
#[path = "api_cases/watchlist.rs"]
mod watchlist;
#[path = "api_cases/woocommerce_webhook.rs"]
mod woocommerce_webhook;
