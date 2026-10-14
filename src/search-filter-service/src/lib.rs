pub mod ports;
mod product_match_evaluator;
mod tier_policy;
pub mod use_cases;

#[cfg(test)]
mod test_digests {
    use notification_service::saved_search_digest::{
        SavedSearchDigestError, SavedSearchDigestMaintenanceReport, SavedSearchDigestStore,
        SavedSearchDigestStoreFactory,
    };
    use time::OffsetDateTime;
    use user_core::user_id::UserId;

    #[derive(Default, Clone)]
    pub struct TestDigests {
        pub checks: std::sync::Arc<std::sync::Mutex<Vec<(UserId, OffsetDateTime)>>>,
    }

    impl<Tx> SavedSearchDigestStoreFactory<Tx> for TestDigests {
        fn in_transaction<'a>(&'a self, _: &'a mut Tx) -> impl SavedSearchDigestStore + 'a {
            self.clone()
        }
    }

    #[async_trait::async_trait]
    impl SavedSearchDigestStore for TestDigests {
        async fn finalize_for_user(
            &mut self,
            user_id: UserId,
            end: OffsetDateTime,
        ) -> Result<Option<SavedSearchDigestMaintenanceReport>, SavedSearchDigestError> {
            self.checks.lock().unwrap().push((user_id, end));
            Ok(None)
        }

        async fn finalize_next(
            &mut self,
        ) -> Result<Option<SavedSearchDigestMaintenanceReport>, SavedSearchDigestError> {
            panic!("completion must finalize only its own user/run")
        }
    }
}
