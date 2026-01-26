use common::{
    api::{
        error::ApiError,
        error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_DOMAIN},
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

pub fn extract_shop_domain_path(path_params: &HashMap<String, String>) -> Result<Domain, ApiError> {
    path_params
        .get("shopDomain")
        .map(|str| format!("\"{str}\""))
        .map(|string| serde_json::from_str(&string))
        .transpose()
        .map_err(|err| {
            let msg = err.to_string();
            ApiError::bad_request(INVALID_DOMAIN, Box::new(err))
                .with_path_field("shopDomain")
                .with_detail(msg)
        })?
        .ok_or(
            ApiError::bad_request(
                BAD_PATH_PARAMETER_VALUE,
                Box::new(MissingRequiredField::new("shopDomain")),
            )
            .with_path_field("shopDomain")
            .with_detail("Missing field 'shopDomain'."),
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
    use crate::data::shop_identifier_data::{ShopIdentifierData, extract_shop_domain_path};
    use common::{
        api::{error::ApiErrorSourceType, error_code::INVALID_DOMAIN},
        domain::Domain,
    };
    use std::collections::HashMap;

    #[rstest::rstest]
    #[case("shop.com", "shop.com".try_into().unwrap())]
    #[case("foo.bar.de", "foo.bar.de".try_into().unwrap())]
    #[case("foo.bar.baz", "foo.bar.baz".try_into().unwrap())]
    fn should_extract_shop_domain_path(#[case] path_param_val: String, #[case] expected: Domain) {
        let path_params = HashMap::from_iter([("shopDomain".to_owned(), path_param_val)]);
        let actual = extract_shop_domain_path(&path_params).unwrap();

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case("-shopcom")]
    #[case("foobarde")]
    #[case("foobarbaz")]
    fn should_err_when_extract_shop_domain_path_for_invalid_domain(#[case] path_param_val: String) {
        let path_params = HashMap::from_iter([("shopDomain".to_owned(), path_param_val)]);
        let actual = extract_shop_domain_path(&path_params).unwrap_err();

        assert_eq!(INVALID_DOMAIN, actual.error);
        assert_eq!(400, actual.status);
        assert_eq!("shopDomain", actual.source.unwrap().field);
        assert_eq!(ApiErrorSourceType::Path, actual.source.unwrap().source_type);
    }

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
}
