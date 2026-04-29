use common::domain::Domain;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct ShopDocumentUpdate {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub domains: Option<HashSet<Domain>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ShopDocumentUpdate {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ShopDocumentUpdate {
                domains: config.fake_with_rng(rng),
                url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::shop_document_update::ShopDocumentUpdate;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_document_update() {
            let _ = Faker.fake::<ShopDocumentUpdate>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::opensearch::{
        shop_document::ShopDocument, shop_document_update::ShopDocumentUpdate,
    };

    #[test]
    fn should_be_subset_of_shop_document() {
        assert!(
            ShopDocumentUpdate::SERDE_FIELDS
                .iter()
                .all(|field| ShopDocument::SERDE_FIELDS.contains(field))
        )
    }
}
