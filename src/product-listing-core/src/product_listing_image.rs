use url::Url;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProductListingImage {
    url: Url,
}

impl ProductListingImage {
    pub fn new(url: Url) -> Self {
        Self { url }
    }

    pub fn url(&self) -> &Url {
        &self.url
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::ProductListingImage;
    use fake::{Dummy, Faker, RngExt};
    use url::Url;

    impl Dummy<Faker> for ProductListingImage {
        fn dummy_with_rng<R: RngExt + ?Sized>(_: &Faker, _: &mut R) -> Self {
            let url = match Url::parse("https://example.com/image.jpg") {
                Ok(url) => url,
                Err(error) => panic!("invalid product image test URL: {error}"),
            };

            ProductListingImage::new(url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProductListingImage;
    use std::collections::HashSet;
    use url::Url;

    #[test]
    fn should_preserve_url_and_equality() -> Result<(), url::ParseError> {
        let url = Url::parse("https://example.test/image.jpg")?;
        let image = ProductListingImage::new(url.clone());

        assert_eq!(image.url(), &url);
        assert!(HashSet::from([image.clone()]).contains(&image));
        Ok(())
    }
}
