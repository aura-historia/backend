//! How many rows one page of a paginated console read holds, shared by the agents' queue
//! (`GET /api/pending-reviews`) and the console's lists. The contract is in
//! `docs/crawler/operations.md` (review service).

/// How many rows one page holds, from 1 to [`PageSize::MAX`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSize(usize);

impl PageSize {
    /// The page size when none is asked for.
    pub const DEFAULT: Self = Self(100);
    /// The largest page.
    pub const MAX: Self = Self(500);

    /// Pages of `size` rows, or `None` when `size` is not between 1 and [`Self::MAX`].
    pub fn new(size: usize) -> Option<Self> {
        (1..=Self::MAX.0).contains(&size).then_some(Self(size))
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_take_page_sizes_from_one_to_the_largest_page() {
        assert_eq!(PageSize::new(0), None);
        assert_eq!(PageSize::new(1).map(PageSize::get), Some(1));
        assert_eq!(PageSize::new(500), Some(PageSize::MAX));
        assert_eq!(PageSize::new(501), None);
        assert!(PageSize::DEFAULT.get() <= PageSize::MAX.get());
    }
}
