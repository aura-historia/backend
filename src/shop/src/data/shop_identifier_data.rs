use common::{
    api::{
        error::ApiError,
        error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_SHOP_IDENTIFIER},
    },
    domain::Domain,
    error::missing_field::MissingRequiredField,
    shop_id::{ShopId, ShopIdentifier},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ShopIdentifierData {
    ShopId(ShopId),
    ShopDomain(Domain),
}

pub fn extract_shop_identifier_data_path(
    path_params: &HashMap<String, String>,
) -> Result<ShopIdentifierData, ApiError> {
    path_params
        .get("shopIdentifier")
        .map(|str| format!("\"{str}\""))
        .map(|string| serde_json::from_str(&string))
        .transpose()
        .map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_SHOP_IDENTIFIER, Box::new(err))
                .with_path_field("shopIdentifier")
                .with_detail(msg)
        })?
        .ok_or(
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("shopIdentifier")),
            )
            .with_path_field("shopIdentifier")
            .with_detail("Missing field 'shopIdentifier'."),
        )
}

impl From<ShopIdentifierData> for ShopIdentifier {
    fn from(data: ShopIdentifierData) -> Self {
        match data {
            ShopIdentifierData::ShopId(shop_id) => ShopIdentifier::ShopId(shop_id),
            ShopIdentifierData::ShopDomain(domain) => ShopIdentifier::ShopDomain(domain),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use common::api::error_code::INVALID_SHOP_IDENTIFIER;
    use fake::{Fake, Faker};

    use crate::data::shop_identifier_data::{
        ShopIdentifierData, extract_shop_identifier_data_path,
    };

    #[rstest::rstest]
    #[case("2a99b5de-cb5e-4c8b-bd06-4a7e3ca3a432", ShopIdentifierData::ShopId("2a99b5de-cb5e-4c8b-bd06-4a7e3ca3a432".try_into().unwrap()))]
    #[case("shop.com", ShopIdentifierData::ShopDomain("shop.com".try_into().unwrap()))]
    #[case("foo.bar.de", ShopIdentifierData::ShopDomain("foo.bar.de".try_into().unwrap()))]
    #[case("foo.bar.baz", ShopIdentifierData::ShopDomain("foo.bar.baz".try_into().unwrap()))]
    fn should_deserialize_shop_identifier_data(
        #[case] payload: &str,
        #[case] expected: ShopIdentifierData,
    ) {
        let actual: ShopIdentifierData = serde_json::from_str(&format!("\"{payload}\"")).unwrap();
        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case("2a99b5de-cb5e-4c8b-bd06-4a7e3ca3a432", ShopIdentifierData::ShopId("2a99b5de-cb5e-4c8b-bd06-4a7e3ca3a432".try_into().unwrap()))]
    #[case("shop.com", ShopIdentifierData::ShopDomain("shop.com".try_into().unwrap()))]
    #[case("foo.bar.de", ShopIdentifierData::ShopDomain("foo.bar.de".try_into().unwrap()))]
    #[case("foo.bar.baz", ShopIdentifierData::ShopDomain("foo.bar.baz".try_into().unwrap()))]
    fn should_serialize_shop_identifier_data(
        #[case] expected: String,
        #[case] payload: ShopIdentifierData,
    ) {
        let actual = serde_json::to_string(&payload).unwrap();

        assert_eq!(format!("\"{expected}\""), actual);
    }

    #[rstest::rstest]
    #[case("2a99b5de-cb5e-4c8b-bd06-4a7e3ca3a432", ShopIdentifierData::ShopId("2a99b5de-cb5e-4c8b-bd06-4a7e3ca3a432".try_into().unwrap()))]
    #[case("shop.com", ShopIdentifierData::ShopDomain("shop.com".try_into().unwrap()))]
    #[case("foo.bar.de", ShopIdentifierData::ShopDomain("foo.bar.de".try_into().unwrap()))]
    #[case("foo.bar.baz", ShopIdentifierData::ShopDomain("foo.bar.baz".try_into().unwrap()))]
    fn should_extract_shop_identifier_data(
        #[case] path_value: String,
        #[case] expected: ShopIdentifierData,
    ) {
        let mut path_params: HashMap<String, String> = Faker.fake();
        path_params.insert("shopIdentifier".to_owned(), path_value);

        let actual = extract_shop_identifier_data_path(&path_params).unwrap();

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case("2a99b5de")]
    #[case("2a99b5de-cb5e")]
    #[case("2a99b5de-cb5e-4c8b-bd06")]
    #[case("2a99b5de-cb5e-4c8b-bd06-4a7e3ca")]
    #[case("norealdomain")]
    #[case("norealdomain:8080")]
    #[case("http://foo")]
    #[case("https://foo")]
    fn should_err_invalid_shop_identifier_when_invalid_for_extract(#[case] path_value: String) {
        let mut path_params: HashMap<String, String> = Faker.fake();
        path_params.insert("shopIdentifier".to_owned(), path_value);

        let actual = extract_shop_identifier_data_path(&path_params).unwrap_err();

        assert_eq!(400, actual.status);
        assert_eq!(INVALID_SHOP_IDENTIFIER, actual.error);
        assert_eq!("shopIdentifier", actual.source.unwrap().field);
    }
}
