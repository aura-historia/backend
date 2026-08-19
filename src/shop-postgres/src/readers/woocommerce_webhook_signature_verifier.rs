use common::error::boxed::box_error;
use common::shop_id::ShopId;
use platform_postgres::SqlxTransaction;
use shop_service::ports::{
    WoocommerceWebhookShopReadError, WoocommerceWebhookSignatureVerification,
    WoocommerceWebhookSignatureVerifier, WoocommerceWebhookSignatureVerifierFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxWoocommerceWebhookSignatureVerifierFactory;

struct SqlxWoocommerceWebhookSignatureVerifier<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(sqlx::FromRow)]
struct WoocommerceWebhookSecretRow {
    woocommerce_webhook_secret: Option<String>,
}

impl SqlxWoocommerceWebhookSignatureVerifierFactory {
    pub fn new() -> Self {
        Self
    }
}

impl WoocommerceWebhookSignatureVerifierFactory<SqlxTransaction>
    for SqlxWoocommerceWebhookSignatureVerifierFactory
{
    fn verifier_in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl WoocommerceWebhookSignatureVerifier + 'tx {
        SqlxWoocommerceWebhookSignatureVerifier {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl WoocommerceWebhookSignatureVerifier for SqlxWoocommerceWebhookSignatureVerifier<'_> {
    async fn verify(
        &mut self,
        shop_id: ShopId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceWebhookSignatureVerification, WoocommerceWebhookShopReadError> {
        let secret = sqlx::query_as::<_, WoocommerceWebhookSecretRow>(
            "SELECT woocommerce_webhook_secret FROM shops WHERE shop_id = $1",
        )
        .bind(uuid::Uuid::from(shop_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(WoocommerceWebhookSignatureSqlxError)?
        .and_then(|row| row.woocommerce_webhook_secret);

        let Some(secret) = secret else {
            return Ok(WoocommerceWebhookSignatureVerification::SecretNotConfigured);
        };
        let expected = hmac_sha256(secret.as_bytes(), body).map_err(|source| {
            WoocommerceWebhookShopReadError::InvalidReadModel {
                source: box_error(source),
            }
        })?;
        Ok(
            if expected.len() == signature.len() && openssl::memcmp::eq(&expected, signature) {
                WoocommerceWebhookSignatureVerification::Valid
            } else {
                WoocommerceWebhookSignatureVerification::Invalid
            },
        )
    }
}

fn hmac_sha256(secret: &[u8], body: &[u8]) -> Result<Vec<u8>, openssl::error::ErrorStack> {
    use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};

    let key = PKey::hmac(secret)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
    signer.update(body)?;
    signer.sign_to_vec()
}

struct WoocommerceWebhookSignatureSqlxError(sqlx::Error);

impl From<WoocommerceWebhookSignatureSqlxError> for WoocommerceWebhookShopReadError {
    fn from(error: WoocommerceWebhookSignatureSqlxError) -> Self {
        Self::TemporarilyUnavailable {
            source: box_error(error.0),
        }
    }
}
