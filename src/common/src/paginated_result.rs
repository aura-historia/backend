use crate::page::Page;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaginatedResult<T, Key> {
    pub items: Vec<T>,
    pub page: Page<Key>,
    pub total: Option<u64>,
    pub next_after: Option<Key>,
}

impl<T, Key> PaginatedResult<T, Key> {
    pub fn map_item<U, F>(self, f: F) -> PaginatedResult<U, Key>
    where
        F: FnMut(T) -> U,
    {
        PaginatedResult {
            items: self.items.into_iter().map(f).collect(),
            page: self.page,
            total: self.total,
            next_after: self.next_after,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::{page::Page, paginated_result::PaginatedResult};
    use fake::{Dummy, Fake, Faker, Rng};
    use time::OffsetDateTime;

    impl<T: Dummy<Faker>> Dummy<Faker> for PaginatedResult<T, u64> {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let items: Vec<T> = config.fake_with_rng(rng);
            let page = Page {
                from: config.fake_with_rng(rng),
                size: config.fake_with_rng(rng),
            };
            PaginatedResult {
                items,
                page,
                total: config.fake_with_rng(rng),
                next_after: config.fake_with_rng(rng),
            }
        }
    }

    impl<T: Dummy<Faker>> Dummy<Faker> for PaginatedResult<T, OffsetDateTime> {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let items: Vec<T> = config.fake_with_rng(rng);
            let page = Page {
                from: OffsetDateTime::now_utc(),
                size: config.fake_with_rng(rng),
            };
            PaginatedResult {
                items,
                page,
                total: config.fake_with_rng(rng),
                next_after: if config.fake_with_rng(rng) {
                    None
                } else {
                    Some(OffsetDateTime::now_utc())
                },
            }
        }
    }
}
