use application::error::box_error;
use geo::{
    GeographicDescription, PostalCode, country_from_code,
    data::geography_data::GeographicDescriptionData, reference::ReferenceRelease,
};
use platform_postgres::SqlxTransaction;
use sqlx::{FromRow, PgConnection};
use user_core::{user_address::*, user_id::UserId};
use user_service::address_book::*;
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
pub struct SqlxAddressBookFactory;
struct Adapter<'a> {
    connection: &'a mut PgConnection,
}
impl AddressBookFactory<SqlxTransaction> for SqlxAddressBookFactory {
    fn access<'a>(&'a self, tx: &'a mut SqlxTransaction) -> impl AddressBookAccess + 'a {
        Adapter {
            connection: tx.connection(),
        }
    }
    fn repository<'a>(&'a self, tx: &'a mut SqlxTransaction) -> impl UserAddressRepository + 'a {
        Adapter {
            connection: tx.connection(),
        }
    }
    fn reader<'a>(&'a self, tx: &'a mut SqlxTransaction) -> impl UserAddressReader + 'a {
        Adapter {
            connection: tx.connection(),
        }
    }
}
fn storage(e: sqlx::Error) -> AddressBookError {
    AddressBookError::Storage(box_error(e))
}
fn invalid(e: impl std::error::Error + Send + Sync + 'static) -> AddressBookError {
    AddressBookError::InvalidState(box_error(e))
}
fn corrupt() -> AddressBookError {
    invalid(std::io::Error::other("invalid persisted user address"))
}
fn number(v: u64) -> Result<i64, AddressBookError> {
    i64::try_from(v)
        .ok()
        .filter(|v| *v > 0)
        .ok_or(AddressBookError::InvalidInput)
}
#[derive(FromRow)]
struct AddressRow {
    address_id: Uuid,
    user_id: Uuid,
    label: String,
    geography: serde_json::Value,
    input_revision: i64,
    version: i64,
    preferred: bool,
}
impl AddressRow {
    fn decode(
        &self,
    ) -> Result<(UserAddressLabel, GeographicDescription, u64, u64), AddressBookError> {
        let data: GeographicDescriptionData =
            serde_json::from_value(self.geography.clone()).map_err(invalid)?;
        let geography: Option<GeographicDescription> = data.try_into().map_err(invalid)?;
        let geography = geography
            .filter(|g| g.address_text().is_some())
            .ok_or_else(corrupt)?;
        let label = UserAddressLabel::new(self.label.clone()).map_err(invalid)?;
        let input_revision = u64::try_from(self.input_revision).map_err(invalid)?;
        let version = u64::try_from(self.version).map_err(invalid)?;
        if input_revision == 0 || version == 0 || input_revision > version {
            return Err(corrupt());
        }
        Ok((label, geography, input_revision, version))
    }
    fn aggregate(self) -> Result<StoredUserAddress, AddressBookError> {
        let (label, geography, input_revision, version) = self.decode()?;
        Ok(StoredUserAddress {
            address: UserAddress::rehydrate(
                UserAddressId::try_from(self.address_id).map_err(invalid)?,
                UserId::try_from(self.user_id).map_err(invalid)?,
                label,
                geography,
                input_revision,
            )
            .map_err(invalid)?,
            version,
        })
    }
    fn view(self) -> Result<UserAddressView, AddressBookError> {
        let (label, geography, input_revision, version) = self.decode()?;
        UserId::try_from(self.user_id).map_err(invalid)?;
        Ok(UserAddressView {
            id: UserAddressId::try_from(self.address_id).map_err(invalid)?,
            label,
            geography,
            input_revision,
            version,
            preferred: self.preferred,
            resolution: UserAddressResolution::unavailable(),
        })
    }
}
#[async_trait::async_trait]
impl AddressBookAccess for Adapter<'_> {
    async fn lock_owner(&mut self, owner: UserId) -> Result<(), AddressBookError> {
        let suspended: Option<bool> =
            sqlx::query_scalar("SELECT suspended FROM users WHERE user_id=$1 FOR NO KEY UPDATE")
                .bind(owner.into_uuid())
                .fetch_optional(&mut *self.connection)
                .await
                .map_err(storage)?;
        match suspended {
            Some(false) => Ok(()),
            Some(true) => Err(AddressBookError::Forbidden),
            None => Err(AddressBookError::NotFound),
        }
    }
}
#[async_trait::async_trait]
impl UserAddressRepository for Adapter<'_> {
    async fn find(
        &mut self,
        owner: UserId,
        id: UserAddressId,
    ) -> Result<Option<StoredUserAddress>, AddressBookError> {
        sqlx::query_as::<_,AddressRow>("SELECT address_id,user_id,label,geography,input_revision,version,false AS preferred FROM user_addresses WHERE user_id=$1 AND address_id=$2 AND NOT deleted")
            .bind(owner.into_uuid()).bind(id.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(storage)?.map(AddressRow::aggregate).transpose()
    }
    async fn insert(
        &mut self,
        address: &UserAddress,
    ) -> Result<StoredUserAddress, AddressBookError> {
        let content = serde_json::to_value(GeographicDescriptionData::from(address.geography()))
            .map_err(invalid)?;
        sqlx::query_as::<_, AddressRow>("INSERT INTO user_addresses(address_id,user_id,label,geography) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING RETURNING address_id,user_id,label,geography,input_revision,version,false AS preferred")
            .bind(address.id().into_uuid()).bind(address.owner().into_uuid()).bind(address.label().as_str()).bind(content).fetch_optional(&mut *self.connection).await.map_err(storage)?
            .ok_or(AddressBookError::Conflict)?.aggregate()
    }
    async fn update(
        &mut self,
        address: &UserAddress,
        expected: u64,
    ) -> Result<StoredUserAddress, AddressBookError> {
        let content = serde_json::to_value(GeographicDescriptionData::from(address.geography()))
            .map_err(invalid)?;
        sqlx::query_as::<_, AddressRow>("UPDATE user_addresses SET label=$3,geography=$4,input_revision=$5,version=version+1 WHERE user_id=$1 AND address_id=$2 AND version=$6 AND version<9223372036854775807 AND NOT deleted RETURNING address_id,user_id,label,geography,input_revision,version,false AS preferred")
            .bind(address.owner().into_uuid()).bind(address.id().into_uuid()).bind(address.label().as_str()).bind(content).bind(number(address.input_revision())?).bind(number(expected)?).fetch_optional(&mut *self.connection).await.map_err(storage)?
            .ok_or(AddressBookError::Conflict)?.aggregate()
    }
    async fn delete(
        &mut self,
        owner: UserId,
        id: UserAddressId,
        expected: u64,
    ) -> Result<(), AddressBookError> {
        // Database erasure hook also protects deletion through the account coordinator.
        let rows = sqlx::query("UPDATE user_addresses SET label=NULL,geography=NULL,deleted=true,version=least(version,9223372036854775806)+1 WHERE user_id=$1 AND address_id=$2 AND version=$3 AND NOT deleted")
            .bind(owner.into_uuid()).bind(id.into_uuid()).bind(number(expected)?).execute(&mut *self.connection).await.map_err(storage)?.rows_affected();
        if rows != 1 {
            return Err(AddressBookError::Conflict);
        }
        Ok(())
    }
    async fn set_destination(
        &mut self,
        owner: UserId,
        destination: &BrowsingDestination,
        expected: u64,
    ) -> Result<(), AddressBookError> {
        let current = UserAddressReader::destination(self, owner).await?;
        if current.version != expected {
            return Err(AddressBookError::Conflict);
        }
        if current.destination == *destination {
            return Ok(());
        }
        if expected == i64::MAX as u64 {
            return Err(AddressBookError::Conflict);
        }
        let (address, country, postal) = match destination {
            BrowsingDestination::None => (None, None, None),
            BrowsingDestination::Address(id) => {
                if UserAddressReader::get(self, owner, *id).await?.is_none() {
                    return Err(AddressBookError::NotFound);
                }
                (Some(id.into_uuid()), None, None)
            }
            BrowsingDestination::CountryPostal {
                country,
                postal_code,
            } => (
                None,
                Some(country.alpha2()),
                postal_code.as_ref().map(|p| p.as_str()),
            ),
        };
        let rows = sqlx::query("INSERT INTO user_browsing_destinations(user_id,address_id,country,postal_code,version) VALUES($1,$2,$3,$4,$5+1) ON CONFLICT(user_id) DO UPDATE SET address_id=EXCLUDED.address_id,country=EXCLUDED.country,postal_code=EXCLUDED.postal_code,version=EXCLUDED.version WHERE user_browsing_destinations.version=$5")
            .bind(owner.into_uuid()).bind(address).bind(country).bind(postal).bind(number(expected)?).execute(&mut *self.connection).await.map_err(storage)?.rows_affected();
        if rows != 1 {
            return Err(AddressBookError::Conflict);
        }
        Ok(())
    }
}
#[derive(FromRow)]
struct DestinationRow {
    address_id: Option<Uuid>,
    country: Option<String>,
    postal_code: Option<String>,
    version: i64,
}
#[async_trait::async_trait]
impl UserAddressReader for Adapter<'_> {
    async fn get(
        &mut self,
        owner: UserId,
        id: UserAddressId,
    ) -> Result<Option<UserAddressView>, AddressBookError> {
        sqlx::query_as::<_,AddressRow>("SELECT a.address_id,a.user_id,a.label,a.geography,a.input_revision,a.version,coalesce(d.address_id=a.address_id,false) AS preferred FROM user_addresses a LEFT JOIN user_browsing_destinations d USING(user_id) WHERE a.user_id=$1 AND a.address_id=$2 AND NOT a.deleted")
            .bind(owner.into_uuid()).bind(id.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(storage)?.map(AddressRow::view).transpose()
    }
    async fn list(
        &mut self,
        owner: UserId,
        after: Option<UserAddressId>,
        limit: u32,
    ) -> Result<UserAddressPage, AddressBookError> {
        if !(1..=100).contains(&limit) {
            return Err(AddressBookError::InvalidInput);
        }
        let rows = sqlx::query_as::<_,AddressRow>("SELECT a.address_id,a.user_id,a.label,a.geography,a.input_revision,a.version,coalesce(d.address_id=a.address_id,false) AS preferred FROM user_addresses a LEFT JOIN user_browsing_destinations d USING(user_id) WHERE a.user_id=$1 AND ($2::uuid IS NULL OR a.address_id>$2) AND NOT a.deleted ORDER BY a.address_id LIMIT $3")
            .bind(owner.into_uuid()).bind(after.map(UserAddressId::into_uuid)).bind(i64::from(limit)+1).fetch_all(&mut *self.connection).await.map_err(storage)?;
        let mut items = rows
            .into_iter()
            .map(AddressRow::view)
            .collect::<Result<Vec<_>, _>>()?;
        let next = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|v| v.id)
        } else {
            None
        };
        Ok(UserAddressPage { items, next })
    }
    async fn destination(&mut self, owner: UserId) -> Result<DestinationView, AddressBookError> {
        let row = sqlx::query_as::<_,DestinationRow>("SELECT address_id,country,postal_code,version FROM user_browsing_destinations WHERE user_id=$1")
            .bind(owner.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(storage)?;
        let Some(row) = row else {
            return Ok(DestinationView {
                destination: BrowsingDestination::None,
                version: 1,
            });
        };
        let version = u64::try_from(row.version).map_err(invalid)?;
        if version == 0 {
            return Err(corrupt());
        }
        let destination = match (row.address_id, row.country, row.postal_code) {
            (None, None, None) => BrowsingDestination::None,
            (Some(id), None, None) => {
                let id = UserAddressId::try_from(id).map_err(invalid)?;
                if self.get(owner, id).await?.is_none() {
                    return Err(corrupt());
                }
                BrowsingDestination::Address(id)
            }
            (None, Some(country), postal) => BrowsingDestination::CountryPostal {
                country: country_from_code(&country, ReferenceRelease::CURRENT).map_err(invalid)?,
                postal_code: postal.map(PostalCode::new).transpose().map_err(invalid)?,
            },
            _ => return Err(corrupt()),
        };
        Ok(DestinationView {
            destination,
            version,
        })
    }
}
