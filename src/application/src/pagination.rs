use domain_primitives::query::range_query::RangeQuery;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

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

/// Continues after the last returned item by its stored sort key and stable ID instead of by
/// looking that item up, so deleting or changing it never ends the traversal early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysetCursor<K, I> {
    pub scope: ScopeBinding,
    pub sort_key: K,
    pub id: I,
}

impl<K, I> KeysetCursor<K, I> {
    pub fn map_sort_key<L>(self, map: impl FnOnce(K) -> L) -> KeysetCursor<L, I> {
        KeysetCursor {
            scope: self.scope,
            sort_key: map(self.sort_key),
            id: self.id,
        }
    }
}

impl<K: SortKey, I> KeysetCursor<K, I> {
    /// Whether this cursor was issued for `scope` and positions by `field`.
    pub fn continues(&self, scope: &ScopeBinding, field: K::Field) -> bool {
        self.scope == *scope && self.sort_key.field() == field
    }
}

/// Stored value of the field a search sorts by.
pub trait SortKey {
    type Field: PartialEq;

    fn field(&self) -> Self::Field;
}

/// Digest of the normalized filter and sort scope a keyset continuation was issued for.
///
/// A continuation is only meaningful for the scope that produced it, so use cases reject one
/// presented with a different scope instead of paging an unrelated result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeBinding([u8; 32]);

impl ScopeBinding {
    /// Starts a binding; `namespace` names the search and its binding format version.
    pub fn builder(namespace: &'static str) -> ScopeBindingBuilder {
        ScopeBindingBuilder(Sha256::new()).part(namespace)
    }

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Feeds scope values in a fixed order. Every part is length-prefixed and absence is tagged,
/// so no two different scopes produce the same input.
#[derive(Debug, Clone)]
pub struct ScopeBindingBuilder(Sha256);

impl ScopeBindingBuilder {
    pub fn part(mut self, value: impl AsRef<[u8]>) -> Self {
        let value = value.as_ref();
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value);
        self
    }

    pub fn optional_part(self, value: Option<impl AsRef<[u8]>>) -> Self {
        match value {
            Some(value) => self.part([1]).part(value),
            None => self.part([0]),
        }
    }

    pub fn timestamp_range(self, range: Option<RangeQuery<OffsetDateTime>>) -> Self {
        let instant = |value: Option<OffsetDateTime>| {
            value.map(|value| value.unix_timestamp_nanos().to_be_bytes())
        };
        match range {
            Some(range) => self
                .part([1])
                .optional_part(instant(range.min))
                .optional_part(instant(range.max)),
            None => self.part([0]),
        }
    }

    /// Binds an unordered set of codes independently of their iteration order.
    pub fn code_set<'a>(self, codes: impl IntoIterator<Item = &'a str>) -> Self {
        let mut codes = codes.into_iter().collect::<Vec<_>>();
        codes.sort_unstable();
        codes.dedup();
        let counted = self.part((codes.len() as u64).to_be_bytes());
        codes
            .into_iter()
            .fold(counted, |builder, code| builder.part(code))
    }

    pub fn finish(self) -> ScopeBinding {
        ScopeBinding(self.0.finalize().into())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(build: impl FnOnce(ScopeBindingBuilder) -> ScopeBindingBuilder) -> ScopeBinding {
        build(ScopeBinding::builder("test-search/v1")).finish()
    }

    #[test]
    fn should_bind_equal_scopes_identically_and_ignore_code_set_order() {
        assert_eq!(
            binding(|scope| scope.optional_part(Some("ada")).code_set(["PRO", "FREE"])),
            binding(|scope| scope
                .optional_part(Some("ada"))
                .code_set(["FREE", "PRO", "FREE"])),
        );
    }

    #[test]
    fn should_distinguish_values_that_concatenate_or_omit_alike() {
        let range =
            |min, max| binding(|scope| scope.timestamp_range(Some(RangeQuery { min, max })));
        for distinct in [
            vec![
                binding(|scope| scope.part("ab").part("c")),
                binding(|scope| scope.part("a").part("bc")),
            ],
            vec![
                binding(|scope| scope.optional_part(None::<&str>)),
                binding(|scope| scope.optional_part(Some(""))),
            ],
            vec![
                binding(|scope| scope.timestamp_range(None)),
                range(None, None),
                range(Some(OffsetDateTime::UNIX_EPOCH), None),
                range(None, Some(OffsetDateTime::UNIX_EPOCH)),
            ],
            vec![
                binding(|scope| scope.code_set([])),
                binding(|scope| scope.code_set(["PRO"])),
                binding(|scope| scope.code_set(["PRO", "FREE"])),
            ],
            vec![
                ScopeBinding::builder("test-search/v1").finish(),
                ScopeBinding::builder("other-search/v1").finish(),
            ],
        ] {
            for (index, binding) in distinct.iter().enumerate() {
                assert!(!distinct[index + 1..].contains(binding));
            }
        }
    }
}
