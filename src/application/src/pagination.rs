#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor<C> {
    pub size: u64,
    pub search_after: Option<C>,
}

impl<C> Default for Cursor<C> {
    fn default() -> Self {
        Self {
            size: 21,
            search_after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursoredResult<T, C> {
    pub items: Vec<T>,
    pub cursor: Cursor<C>,
    pub total: Option<u64>,
}

impl<T, C> CursoredResult<T, C> {
    pub fn map_item<U, F>(self, f: F) -> CursoredResult<U, C>
    where
        F: FnMut(T) -> U,
    {
        CursoredResult {
            items: self.items.into_iter().map(f).collect(),
            cursor: self.cursor,
            total: self.total,
        }
    }
}

impl<T, C> Default for CursoredResult<T, C> {
    fn default() -> Self {
        Self {
            items: vec![],
            cursor: Default::default(),
            total: None,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::{Cursor, CursoredResult};
    use domain_primitives::event_id::EventId;
    use fake::{Dummy, Fake, Faker, RngExt};
    use time::OffsetDateTime;

    impl<T: Dummy<Faker>> Dummy<Faker> for CursoredResult<T, OffsetDateTime> {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Self {
                items: config.fake_with_rng(rng),
                cursor: Cursor {
                    search_after: if config.fake_with_rng(rng) {
                        Some(OffsetDateTime::now_utc())
                    } else {
                        None
                    },
                    size: config.fake_with_rng(rng),
                },
                total: config.fake_with_rng(rng),
            }
        }
    }

    impl<T: Dummy<Faker>> Dummy<Faker> for CursoredResult<T, EventId> {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Self {
                items: config.fake_with_rng(rng),
                cursor: Cursor {
                    search_after: if config.fake_with_rng::<bool, _>(rng) {
                        Some(EventId::new())
                    } else {
                        None
                    },
                    size: config.fake_with_rng(rng),
                },
                total: config.fake_with_rng(rng),
            }
        }
    }
}
