use application::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName};
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation, WoocommerceIntegration};
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_postgres::{
    SqlxShopRepositoryFactory, SqlxWoocommerceWebhookShopReaderFactory,
    SqlxWoocommerceWebhookSignatureVerifierFactory,
};
use shop_service::ports::{
    ShopRepository, ShopRepositoryFactory, WoocommerceWebhookShopReader,
    WoocommerceWebhookShopReaderFactory, WoocommerceWebhookSignatureVerification,
    WoocommerceWebhookSignatureVerifier, WoocommerceWebhookSignatureVerifierFactory,
};
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_safe_woocommerce_webhook_validation_configuration() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let repositories = SqlxShopRepositoryFactory::new();
    let webhooks = SqlxWoocommerceWebhookShopReaderFactory::new();
    let shop = shop("WooCommerce Safe Reader Shop", Some("secret"));

    let mut tx = match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    };
    if let Err(error) = repositories.in_transaction(&mut tx).insert(&shop).await {
        panic!("failed to insert shop: {error:?}");
    }
    let result = webhooks
        .in_transaction(&mut tx)
        .find_for_webhook(shop.id())
        .await;
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }

    let webhook_shop = match result {
        Ok(Some(shop)) => shop,
        Ok(None) => panic!("missing WooCommerce webhook shop"),
        Err(error) => panic!("failed to read WooCommerce webhook shop: {error:?}"),
    };
    assert_eq!(shop.id(), webhook_shop.shop_id);
    assert_eq!(ShopPartnerStatus::Partnered, webhook_shop.partner_status);
    assert_eq!(Some(Currency::Eur), webhook_shop.currency);
    assert_eq!(Some(Language::En), webhook_shop.language);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_verify_woocommerce_webhook_signature_without_returning_secret() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let repositories = SqlxShopRepositoryFactory::new();
    let verifier = SqlxWoocommerceWebhookSignatureVerifierFactory::new();
    let configured_shop = shop("WooCommerce Configured Verifier Shop", Some("key"));
    let unconfigured_shop = shop("WooCommerce Unconfigured Verifier Shop", None);
    let body = b"The quick brown fox jumps over the lazy dog";
    let valid_signature = [
        0xf7, 0xbc, 0x83, 0xf4, 0x30, 0x53, 0x84, 0x24, 0xb1, 0x32, 0x98, 0xe6, 0xaa, 0x6f, 0xb1,
        0x43, 0xef, 0x4d, 0x59, 0xa1, 0x49, 0x46, 0x17, 0x59, 0x97, 0x47, 0x9d, 0xbc, 0x2d, 0x1a,
        0x3c, 0xd8,
    ];

    let mut tx = match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    };
    for shop in [&configured_shop, &unconfigured_shop] {
        if let Err(error) = repositories.in_transaction(&mut tx).insert(shop).await {
            panic!("failed to insert shop: {error:?}");
        }
    }
    let valid = verifier
        .verifier_in_transaction(&mut tx)
        .verify(configured_shop.id(), body, &valid_signature)
        .await;
    let invalid = verifier
        .verifier_in_transaction(&mut tx)
        .verify(configured_shop.id(), body, b"invalid")
        .await;
    let unconfigured = verifier
        .verifier_in_transaction(&mut tx)
        .verify(unconfigured_shop.id(), body, &valid_signature)
        .await;
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }

    assert!(matches!(
        valid,
        Ok(WoocommerceWebhookSignatureVerification::Valid)
    ));
    assert!(matches!(
        invalid,
        Ok(WoocommerceWebhookSignatureVerification::Invalid)
    ));
    assert!(matches!(
        unconfigured,
        Ok(WoocommerceWebhookSignatureVerification::SecretNotConfigured)
    ));
}

fn shop(name: &str, secret: Option<&str>) -> Shop {
    Shop::create(NewShop {
        id: ShopId::new(),
        name: ShopName::from(name),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::new(),
        shopify: None,
        woocommerce: Some(WoocommerceIntegration {
            webhook_secret: secret.map(WoocommerceWebhookSecret::from),
            currency: Some(Currency::Eur),
            language: Some(Language::En),
        }),
        presentation: ShopPresentation::default(),
        address: None,
        contact: ShopContact::default(),
        partner_status: ShopPartnerStatus::Partnered,
        affiliate_configuration: None,
    })
}
