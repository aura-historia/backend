//! Public location-oriented discovery. Protected location state is not a read model.
use application::{
    error::{BoxError, box_error},
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, UnitOfWork},
};
use geo::{CountryCode, SpatialPosition, SubdivisionCode, core::discovery::GeographicQuery};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverPartyLocationsRequest {
    pub geography: GeographicQuery,
    pub cursor: Cursor<PartyLocationDiscoveryCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartyLocationDiscoveryCursor {
    pub location_id: PartyLocationId,
    pub distance_km: Option<f64>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PublicPartyLocation {
    pub party_id: PartyId,
    pub party_name: String,
    pub location_id: PartyLocationId,
    pub label: String,
    pub country: Option<CountryCode>,
    pub subdivision: Option<SubdivisionCode>,
    pub position: Option<SpatialPosition>,
    pub basis: &'static str,
    pub distance_km: Option<f64>,
}
pub type DiscoverPartyLocationsResult =
    CursoredResult<PublicPartyLocation, PartyLocationDiscoveryCursor>;
#[derive(Debug, thiserror::Error)]
pub enum DiscoverPartyLocationsError {
    #[error("invalid location discovery query or cursor")]
    InvalidInput,
    #[error("location discovery failed")]
    Read(#[source] BoxError),
    #[error("invalid location discovery state")]
    InvalidState(#[source] BoxError),
    /// The selected page and current eligible resolution evidence disagree.
    #[error("inconsistent location discovery geography snapshots")]
    InconsistentSnapshot,
    #[error("location discovery transaction failed")]
    Transaction(
        #[from]
        #[source]
        application::transaction::TransactionError,
    ),
}
#[async_trait::async_trait]
pub trait PartyLocationDiscoveryReader: Send {
    async fn discover(
        &mut self,
        request: &DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError>;
}
pub trait PartyLocationDiscoveryReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyLocationDiscoveryReader + 'tx;
}
#[async_trait::async_trait]
pub trait DiscoverPartyLocationsUseCase: Send + Sync {
    async fn execute(
        &self,
        request: DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError>;
}
pub struct DiscoverPartyLocationsHandler<U, R> {
    uow: U,
    reader: R,
}
impl<U, R> DiscoverPartyLocationsHandler<U, R> {
    pub fn new(uow: U, reader: R) -> Self {
        Self { uow, reader }
    }
}
#[async_trait::async_trait]
impl<U, R> DiscoverPartyLocationsUseCase for DiscoverPartyLocationsHandler<U, R>
where
    U: UnitOfWork,
    R: PartyLocationDiscoveryReaderFactory<U::Tx>,
{
    async fn execute(
        &self,
        request: DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
        let ordered = request.geography.distance_order().is_some();
        if !(1..=100).contains(&request.cursor.size)
            || request.cursor.search_after.is_some_and(|c| {
                c.distance_km.is_some() != ordered
                    || c.distance_km.is_some_and(|d| !d.is_finite() || d < 0.0)
            })
        {
            return Err(DiscoverPartyLocationsError::InvalidInput);
        }
        let mut retried = false;
        loop {
            let mut tx = self.uow.begin().await?;
            let result = self.reader.in_transaction(&mut tx).discover(&request).await;
            match result {
                Err(DiscoverPartyLocationsError::InconsistentSnapshot) if !retried => {
                    // Release evidence locks and rerun the entire original page. Skipping
                    // changed hits could lose matches or produce an incorrect cursor.
                    drop(tx);
                    retried = true;
                }
                Err(error @ DiscoverPartyLocationsError::InconsistentSnapshot) => {
                    return Err(DiscoverPartyLocationsError::InvalidState(box_error(error)));
                }
                Err(error) => return Err(error),
                Ok(result) => {
                    tx.commit().await?;
                    return Ok(result);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Transactions {
        begins: AtomicUsize,
        active: AtomicBool,
    }
    struct TestUnitOfWork(Arc<Transactions>);
    struct TestTransaction(Arc<Transactions>);
    impl Drop for TestTransaction {
        fn drop(&mut self) {
            assert!(self.0.active.swap(false, Ordering::SeqCst));
        }
    }
    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), application::transaction::TransactionError> {
            panic!("a failed discovery must not commit")
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;
        async fn begin(&self) -> Result<Self::Tx, application::transaction::TransactionError> {
            assert!(!self.0.active.swap(true, Ordering::SeqCst));
            self.0.begins.fetch_add(1, Ordering::SeqCst);
            Ok(TestTransaction(self.0.clone()))
        }
    }

    #[derive(Clone, Copy)]
    enum FailureReader {
        Snapshot,
        Corrupt,
    }
    impl PartyLocationDiscoveryReaderFactory<TestTransaction> for FailureReader {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl PartyLocationDiscoveryReader + 'tx {
            *self
        }
    }
    #[async_trait::async_trait]
    impl PartyLocationDiscoveryReader for FailureReader {
        async fn discover(
            &mut self,
            _request: &DiscoverPartyLocationsRequest,
        ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
            Err(match self {
                Self::Snapshot => DiscoverPartyLocationsError::InconsistentSnapshot,
                Self::Corrupt => DiscoverPartyLocationsError::InvalidState(
                    application::error::static_error("invalid persisted geography"),
                ),
            })
        }
    }
    fn request() -> DiscoverPartyLocationsRequest {
        DiscoverPartyLocationsRequest {
            geography: GeographicQuery::new(
                geo::reference::ReferenceRelease::CURRENT,
                std::collections::HashSet::from([CountryCode::DEU]),
                Default::default(),
                None,
                None,
                Default::default(),
                None,
            )
            .unwrap(),
            cursor: Cursor {
                size: 1,
                search_after: None,
            },
        }
    }

    #[tokio::test]
    async fn persistent_snapshot_disagreement_fails_closed_after_one_fresh_retry() {
        let transactions = Arc::new(Transactions::default());
        let handler = DiscoverPartyLocationsHandler::new(
            TestUnitOfWork(transactions.clone()),
            FailureReader::Snapshot,
        );
        assert!(matches!(
            handler.execute(request()).await,
            Err(DiscoverPartyLocationsError::InvalidState(_))
        ));
        assert_eq!(2, transactions.begins.load(Ordering::SeqCst));
        assert!(!transactions.active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn corrupt_state_is_returned_without_retry() {
        let transactions = Arc::new(Transactions::default());
        let handler = DiscoverPartyLocationsHandler::new(
            TestUnitOfWork(transactions.clone()),
            FailureReader::Corrupt,
        );
        let Err(DiscoverPartyLocationsError::InvalidState(cause)) =
            handler.execute(request()).await
        else {
            panic!("corrupt state must remain an error")
        };
        assert_eq!("invalid persisted geography", cause.to_string());
        assert_eq!(1, transactions.begins.load(Ordering::SeqCst));
        assert!(!transactions.active.load(Ordering::SeqCst));
    }
}
