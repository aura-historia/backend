//! Which rows of a console list one request reads, and the envelope that answers it. The contract
//! is in `docs/crawler/operations.md` (review service).

use serde::Serialize;

/// The rows `offset` to `offset + limit` of a list in its order. The window is the one fixed page
/// of the console's pager, 100 rows, unless the request asks for fewer or up to
/// [`ListWindow::MAX_LIMIT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListWindow {
    limit: usize,
    offset: usize,
}

impl ListWindow {
    pub const DEFAULT_LIMIT: usize = 100;
    pub const MAX_LIMIT: usize = 200;

    /// The window of `limit` rows from `offset`, or `None` when `limit` is not between 1 and
    /// [`Self::MAX_LIMIT`].
    pub fn new(limit: usize, offset: usize) -> Option<Self> {
        (1..=Self::MAX_LIMIT)
            .contains(&limit)
            .then_some(Self { limit, offset })
    }

    pub const fn limit(self) -> usize {
        self.limit
    }

    pub const fn offset(self) -> usize {
        self.offset
    }
}

impl Default for ListWindow {
    fn default() -> Self {
        Self {
            limit: Self::DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

/// One window of a console list and how many rows match its query in all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListPage<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}

impl<T> ListPage<T> {
    pub fn new(items: Vec<T>, total: i64, window: ListWindow) -> Self {
        Self {
            items,
            total,
            limit: window.limit(),
            offset: window.offset(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_take_windows_of_one_to_two_hundred_rows() {
        assert_eq!(ListWindow::new(0, 0), None);
        assert_eq!(ListWindow::new(201, 0), None);
        assert_eq!(ListWindow::new(200, 400).map(ListWindow::offset), Some(400));
        assert_eq!(ListWindow::default().limit(), 100);
    }
}
