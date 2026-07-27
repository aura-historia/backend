#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Versioned<T, V> {
    pub value: T,
    pub version: V,
}

impl<T, V> Versioned<T, V> {
    pub fn new(value: T, version: V) -> Self {
        Self { value, version }
    }

    pub fn into_value(self) -> T {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_wrap_value_with_version() {
        let loaded = Versioned::new("shop", 3_u64);

        assert_eq!("shop", loaded.value);
        assert_eq!(3, loaded.version);
    }

    #[test]
    fn should_return_value_when_unwrapping() {
        let loaded = Versioned::new("shop", 3_u64);

        assert_eq!("shop", loaded.into_value());
    }
}
