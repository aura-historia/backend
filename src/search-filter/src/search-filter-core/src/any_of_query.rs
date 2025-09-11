use std::{
    collections::HashSet,
    fmt,
    hash::Hash,
    ops::{Deref, DerefMut},
};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
pub struct AnyOfQuery<T: Eq + Hash>(HashSet<T>);

impl<T: Eq + Hash> Default for AnyOfQuery<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<T: Eq + Hash> Deref for AnyOfQuery<T> {
    type Target = HashSet<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Eq + Hash> DerefMut for AnyOfQuery<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Eq + Hash> AsRef<HashSet<T>> for AnyOfQuery<T> {
    fn as_ref(&self) -> &HashSet<T> {
        &self.0
    }
}

impl<T: Eq + Hash> AsMut<HashSet<T>> for AnyOfQuery<T> {
    fn as_mut(&mut self) -> &mut HashSet<T> {
        &mut self.0
    }
}

impl<T: Eq + Hash> IntoIterator for AnyOfQuery<T> {
    type Item = T;
    type IntoIter = std::collections::hash_set::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T: Eq + Hash> IntoIterator for &'a AnyOfQuery<T> {
    type Item = &'a T;
    type IntoIter = std::collections::hash_set::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Eq + Hash> From<HashSet<T>> for AnyOfQuery<T> {
    fn from(set: HashSet<T>) -> Self {
        Self(set)
    }
}

impl<T: Eq + Hash> From<AnyOfQuery<T>> for HashSet<T> {
    fn from(query: AnyOfQuery<T>) -> Self {
        query.0
    }
}

impl<T: Eq + Hash> FromIterator<T> for AnyOfQuery<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<T: Eq + Hash> Extend<T> for AnyOfQuery<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl<'a, T: Eq + Hash + Copy> Extend<&'a T> for AnyOfQuery<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        self.0.extend(iter.into_iter().copied());
    }
}

impl<T: Eq + Hash + fmt::Debug> fmt::Display for AnyOfQuery<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnyOfQuery{:?}", self.0)
    }
}
