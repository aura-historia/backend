use crate::mapping::{
    UserRow, bind_country, bind_currency, bind_language, bind_measurement_unit, bind_role,
    bind_tier, user_columns, version_to_i64,
};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use serde_email::Email;
use sqlx::{AssertSqlSafe, PgConnection};
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user::User;
use user_core::user_id::UserId;
use user_service::ports::{
    UserInsertOutcome, UserRepository, UserRepositoryError, UserRepositoryFactory,
    UserStorageVersion, VersionedUser,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserRepositoryFactory;

struct SqlxUserRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxUserRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserRepositoryFactory<SqlxTransaction> for SqlxUserRepositoryFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl UserRepository + 'tx {
        SqlxUserRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserRepository for SqlxUserRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: UserId,
    ) -> Result<Option<VersionedUser>, UserRepositoryError> {
        let sql = format!("SELECT {} FROM users WHERE user_id = $1", user_columns());
        let row = sqlx::query_as::<_, UserRow>(AssertSqlSafe(sql))
            .bind(uuid::Uuid::from(id))
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(|source| UserRepositoryError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        row.map(VersionedUser::try_from)
            .transpose()
            .map_err(|source| UserRepositoryError::InvalidPersistedState {
                source: box_error(source),
            })
    }

    async fn find_by_email(
        &mut self,
        email: &Email,
    ) -> Result<Option<VersionedUser>, UserRepositoryError> {
        let sql = format!("SELECT {} FROM users WHERE email = $1", user_columns());
        let row = sqlx::query_as::<_, UserRow>(AssertSqlSafe(sql))
            .bind::<&str>(email.as_ref())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(|source| UserRepositoryError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        row.map(VersionedUser::try_from)
            .transpose()
            .map_err(|source| UserRepositoryError::InvalidPersistedState {
                source: box_error(source),
            })
    }

    async fn find_by_stripe_customer_id(
        &mut self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Option<VersionedUser>, UserRepositoryError> {
        let sql = format!(
            "SELECT {} FROM users WHERE stripe_customer_id = $1",
            user_columns()
        );
        let row = sqlx::query_as::<_, UserRow>(AssertSqlSafe(sql))
            .bind(stripe_customer_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(|source| UserRepositoryError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        row.map(VersionedUser::try_from)
            .transpose()
            .map_err(|source| UserRepositoryError::InvalidPersistedState {
                source: box_error(source),
            })
    }

    async fn insert(&mut self, user: &User) -> Result<VersionedUser, UserRepositoryError> {
        let profile = user.profile();
        let preferences = user.preferences();
        let account = user.account();
        let structured_address = profile.structured_address.as_ref();
        let geo_address = profile.geo_address;

        let sql = format!(
            r#"
            INSERT INTO users (
                user_id, email, first_name, last_name, language, currency, measurement_unit,
                prohibited_content_consent, tier, role, stripe_customer_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region,
                structured_address_postal_code, structured_address_country,
                geo_address_lat, geo_address_lon
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7,
                $8, $9, $10, $11,
                $12, $13, $14, $15, $16, $17,
                $18, $19
            )
            RETURNING {}
            "#,
            user_columns()
        );

        let row = sqlx::query_as::<_, UserRow>(AssertSqlSafe(sql))
            .bind(uuid::Uuid::from(user.id()))
            .bind::<&str>(user.email().as_ref())
            .bind(profile.first_name.as_ref().map(AsRef::as_ref))
            .bind(profile.last_name.as_ref().map(AsRef::as_ref))
            .bind(bind_language(preferences.language))
            .bind(bind_currency(preferences.currency))
            .bind(bind_measurement_unit(preferences.measurement_unit))
            .bind(preferences.prohibited_content_consent)
            .bind(bind_tier(account.tier))
            .bind(bind_role(account.role))
            .bind(account.stripe_customer_id.as_ref().map(AsRef::as_ref))
            .bind(structured_address.and_then(|value| value.addressline.as_deref()))
            .bind(structured_address.and_then(|value| value.addressline_extra.as_deref()))
            .bind(structured_address.and_then(|value| value.locality.as_deref()))
            .bind(structured_address.and_then(|value| value.region.as_deref()))
            .bind(structured_address.and_then(|value| value.postal_code.as_deref()))
            .bind(bind_country(structured_address))
            .bind(geo_address.map(|value| value.lat))
            .bind(geo_address.map(|value| value.lon))
            .fetch_one(&mut *self.connection)
            .await
            .map_err(map_write_error)?;

        VersionedUser::try_from(row).map_err(|source| UserRepositoryError::InvalidPersistedState {
            source: box_error(source),
        })
    }

    async fn insert_if_absent(
        &mut self,
        user: &User,
    ) -> Result<UserInsertOutcome, UserRepositoryError> {
        let profile = user.profile();
        let preferences = user.preferences();
        let account = user.account();
        let structured_address = profile.structured_address.as_ref();
        let geo_address = profile.geo_address;

        let sql = format!(
            r#"
            INSERT INTO users (
                user_id, email, first_name, last_name, language, currency, measurement_unit,
                prohibited_content_consent, tier, role, stripe_customer_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region,
                structured_address_postal_code, structured_address_country,
                geo_address_lat, geo_address_lon
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7,
                $8, $9, $10, $11,
                $12, $13, $14, $15, $16, $17,
                $18, $19
            )
            ON CONFLICT (user_id) DO NOTHING
            RETURNING {}
            "#,
            user_columns()
        );

        let inserted = sqlx::query_as::<_, UserRow>(AssertSqlSafe(sql))
            .bind(uuid::Uuid::from(user.id()))
            .bind::<&str>(user.email().as_ref())
            .bind(profile.first_name.as_ref().map(AsRef::as_ref))
            .bind(profile.last_name.as_ref().map(AsRef::as_ref))
            .bind(bind_language(preferences.language))
            .bind(bind_currency(preferences.currency))
            .bind(bind_measurement_unit(preferences.measurement_unit))
            .bind(preferences.prohibited_content_consent)
            .bind(bind_tier(account.tier))
            .bind(bind_role(account.role))
            .bind(account.stripe_customer_id.as_ref().map(AsRef::as_ref))
            .bind(structured_address.and_then(|value| value.addressline.as_deref()))
            .bind(structured_address.and_then(|value| value.addressline_extra.as_deref()))
            .bind(structured_address.and_then(|value| value.locality.as_deref()))
            .bind(structured_address.and_then(|value| value.region.as_deref()))
            .bind(structured_address.and_then(|value| value.postal_code.as_deref()))
            .bind(bind_country(structured_address))
            .bind(geo_address.map(|value| value.lat))
            .bind(geo_address.map(|value| value.lon))
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(map_write_error)?;

        match inserted {
            Some(row) => VersionedUser::try_from(row)
                .map(UserInsertOutcome::Created)
                .map_err(|source| UserRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }),
            None => self
                .find_by_id(user.id())
                .await?
                .map(UserInsertOutcome::Existing)
                .ok_or_else(|| UserRepositoryError::TemporarilyUnavailable {
                    source: box_error(std::io::Error::other(
                        "user disappeared after idempotent insert conflict",
                    )),
                }),
        }
    }

    async fn update(
        &mut self,
        user: &User,
        expected_version: UserStorageVersion,
    ) -> Result<VersionedUser, UserRepositoryError> {
        let profile = user.profile();
        let preferences = user.preferences();
        let account = user.account();
        let structured_address = profile.structured_address.as_ref();
        let geo_address = profile.geo_address;

        let sql = format!(
            r#"
            UPDATE users SET
                email = $2,
                first_name = $3,
                last_name = $4,
                language = $5,
                currency = $6,
                measurement_unit = $7,
                prohibited_content_consent = $8,
                tier = $9,
                role = $10,
                stripe_customer_id = $11,
                structured_address_addressline = $12,
                structured_address_addressline_extra = $13,
                structured_address_locality = $14,
                structured_address_region = $15,
                structured_address_postal_code = $16,
                structured_address_country = $17,
                geo_address_lat = $18,
                geo_address_lon = $19,
                version = version + 1,
                updated = now()
            WHERE user_id = $1 AND version = $20
            RETURNING {}
            "#,
            user_columns()
        );

        let row = sqlx::query_as::<_, UserRow>(AssertSqlSafe(sql))
            .bind(uuid::Uuid::from(user.id()))
            .bind::<&str>(user.email().as_ref())
            .bind(profile.first_name.as_ref().map(AsRef::as_ref))
            .bind(profile.last_name.as_ref().map(AsRef::as_ref))
            .bind(bind_language(preferences.language))
            .bind(bind_currency(preferences.currency))
            .bind(bind_measurement_unit(preferences.measurement_unit))
            .bind(preferences.prohibited_content_consent)
            .bind(bind_tier(account.tier))
            .bind(bind_role(account.role))
            .bind(account.stripe_customer_id.as_ref().map(AsRef::as_ref))
            .bind(structured_address.and_then(|value| value.addressline.as_deref()))
            .bind(structured_address.and_then(|value| value.addressline_extra.as_deref()))
            .bind(structured_address.and_then(|value| value.locality.as_deref()))
            .bind(structured_address.and_then(|value| value.region.as_deref()))
            .bind(structured_address.and_then(|value| value.postal_code.as_deref()))
            .bind(bind_country(structured_address))
            .bind(geo_address.map(|value| value.lat))
            .bind(geo_address.map(|value| value.lon))
            .bind(version_to_i64(expected_version))
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(map_write_error)?
            .ok_or(UserRepositoryError::ConcurrencyConflict)?;

        VersionedUser::try_from(row).map_err(|source| UserRepositoryError::InvalidPersistedState {
            source: box_error(source),
        })
    }

    async fn delete_by_id(&mut self, id: UserId) -> Result<bool, UserRepositoryError> {
        let result = sqlx::query("DELETE FROM users WHERE user_id = $1")
            .bind(uuid::Uuid::from(id))
            .execute(&mut *self.connection)
            .await
            .map_err(map_write_error)?;

        Ok(result.rows_affected() > 0)
    }
}

fn map_write_error(source: sqlx::Error) -> UserRepositoryError {
    if let sqlx::Error::Database(database_error) = &source
        && database_error.is_unique_violation()
    {
        let constraint = database_error.constraint().unwrap_or_default();
        if constraint.contains("stripe_customer") {
            return UserRepositoryError::StripeCustomerConflict {
                source: box_error(source),
            };
        }
        return UserRepositoryError::EmailConflict {
            source: box_error(source),
        };
    }

    UserRepositoryError::TemporarilyUnavailable {
        source: box_error(source),
    }
}
