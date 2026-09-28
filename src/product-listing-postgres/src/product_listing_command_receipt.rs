use application::error::box_error;
use listing_source_core::ListingSourceId;
use product_listing_service::{
    ports::{
        ProductListingCommandCompletionCode, ProductListingCommandReceipt,
        ProductListingCommandReceiptError, ProductListingCommandReceiptStore,
        ProductListingCommandReceiptStoreFactory, ProductListingCommandReceiptWrite,
    },
    use_cases::commands::product_listing_ingestion::{
        ProductListingIngestionFingerprint, ProductListingIngestionOperation,
    },
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingCommandReceiptStoreFactory;

struct SqlxProductListingCommandReceiptStore<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(sqlx::FromRow)]
struct ReceiptRow {
    submission_id: String,
    listing_source_id: uuid::Uuid,
    operation: String,
    semantic_fingerprint: Vec<u8>,
    completion_code: String,
    completed_at: time::OffsetDateTime,
}

impl SqlxProductListingCommandReceiptStoreFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingCommandReceiptStoreFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingCommandReceiptStoreFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingCommandReceiptStore + 'tx {
        SqlxProductListingCommandReceiptStore {
            connection: tx.connection(),
        }
    }
}

impl SqlxProductListingCommandReceiptStore<'_> {
    async fn lock(&mut self, command_id: &str) -> Result<(), ProductListingCommandReceiptError> {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('product_listing_command_receipt:' || $1, 0))",
        )
        .bind(command_id)
        .execute(&mut *self.connection)
        .await
        .map_err(operation_failed)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ProductListingCommandReceiptStore for SqlxProductListingCommandReceiptStore<'_> {
    async fn lock_and_find(
        &mut self,
        command_id: &str,
    ) -> Result<Option<ProductListingCommandReceipt>, ProductListingCommandReceiptError> {
        if !valid_id(command_id, "plic1_") {
            return Err(ProductListingCommandReceiptError::InvalidIdentity);
        }
        self.lock(command_id).await?;
        let row = sqlx::query_as::<_, ReceiptRow>(
            "SELECT submission_id, listing_source_id, operation, semantic_fingerprint, completion_code, completed_at FROM product_listing_command_receipts WHERE command_id = $1",
        )
        .bind(command_id)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(operation_failed)?;
        row.map(decode_receipt).transpose()
    }

    async fn insert(
        &mut self,
        receipt: &ProductListingCommandReceiptWrite,
    ) -> Result<(), ProductListingCommandReceiptError> {
        if !valid_id(&receipt.command_id, "plic1_") || !valid_id(&receipt.submission_id, "plis1_") {
            return Err(ProductListingCommandReceiptError::InvalidIdentity);
        }
        // Also acquire the lock when called without a prior lookup. The unique key is the
        // ultimate duplicate fence, while this lock serializes lookup/write on an absent row.
        self.lock(&receipt.command_id).await?;
        sqlx::query(
            "INSERT INTO product_listing_command_receipts (command_id, submission_id, listing_source_id, operation, semantic_fingerprint, completion_code) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&receipt.command_id)
        .bind(&receipt.submission_id)
        .bind(receipt.listing_source_id.as_uuid())
        .bind(receipt.operation.as_str())
        .bind(receipt.fingerprint.as_bytes().as_slice())
        .bind(ProductListingCommandCompletionCode::Applied.as_str())
        .execute(&mut *self.connection)
        .await
        .map_err(|error| match &error {
            sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
                ProductListingCommandReceiptError::AlreadyExists
            }
            _ => operation_failed(error),
        })?;
        Ok(())
    }
}

fn decode_receipt(
    row: ReceiptRow,
) -> Result<ProductListingCommandReceipt, ProductListingCommandReceiptError> {
    use ProductListingCommandReceiptError::InvalidPersistedState as Invalid;
    let digest: [u8; 32] = row.semantic_fingerprint.try_into().map_err(|_| Invalid)?;
    let listing_source_id =
        ListingSourceId::try_from(row.listing_source_id).map_err(|_| Invalid)?;
    if !valid_id(&row.submission_id, "plis1_") {
        return Err(Invalid);
    }
    let operation = match row.operation.as_str() {
        "CREATE" => ProductListingIngestionOperation::Create,
        "UPDATE" => ProductListingIngestionOperation::Update,
        "UPSERT" => ProductListingIngestionOperation::Upsert,
        "WITHDRAW" => ProductListingIngestionOperation::Withdraw,
        "CAPTURE_RAW" => ProductListingIngestionOperation::CaptureRaw,
        _ => return Err(Invalid),
    };
    let completion_code = match row.completion_code.as_str() {
        "APPLIED" => ProductListingCommandCompletionCode::Applied,
        _ => return Err(Invalid),
    };
    Ok(ProductListingCommandReceipt {
        submission_id: row.submission_id,
        listing_source_id,
        operation,
        fingerprint: ProductListingIngestionFingerprint::from_digest(digest),
        completion_code,
        completed_at: row.completed_at,
    })
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn operation_failed(error: sqlx::Error) -> ProductListingCommandReceiptError {
    ProductListingCommandReceiptError::OperationFailed {
        source: box_error(error),
    }
}
