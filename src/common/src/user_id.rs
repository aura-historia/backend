use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct UserId(Uuid);

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Display for UserId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for UserId {
    fn from(uuid: Uuid) -> Self {
        UserId(uuid)
    }
}

impl TryFrom<String> for UserId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<UserId> for String {
    fn from(id: UserId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for UserId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{INTERNAL_SERVER_ERROR, UNAUTHORIZED},
        },
        user_id::UserId,
    };
    use aws_lambda_events::apigw::ApiGatewayV2httpRequestContext;

    pub fn extract_user_id_request_context(
        request_context: &ApiGatewayV2httpRequestContext,
    ) -> Result<UserId, ApiError> {
        let user_id = request_context
            .authorizer
            .as_ref()
            .ok_or_else(|| {
                ApiError::unauthorized(UNAUTHORIZED)
                    .with_header_field("Authorization")
                    .with_detail("Missing authorizer-information in request-context.")
            })?
            .jwt
            .as_ref()
            .ok_or_else(|| {
                ApiError::unauthorized(UNAUTHORIZED)
                    .with_header_field("Authorization")
                    .with_detail("Missing JWT.")
            })?
            .claims
            .get("sub")
            .ok_or_else(|| {
                ApiError::internal_server_error(
                    INTERNAL_SERVER_ERROR,
                    "Missing claim 'sub' in Cognito-authorized JWT.".into(),
                )
            })
            .map(String::as_str)
            .map(UserId::try_from)?
            .map_err(|err| ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err)))?;

        Ok(user_id)
    }

    #[cfg(test)]
    mod tests {
        use rstest;

        use crate::user_id::api::extract_user_id_request_context;
        use aws_lambda_events::apigw::{
            ApiGatewayRequestAuthorizer, ApiGatewayRequestAuthorizerJwtDescription,
            ApiGatewayV2httpRequestContext,
        };
        use std::collections::HashMap;
        use uuid::Uuid;

        fn create_jwt_description_with_claims(
            claims: HashMap<String, String>,
        ) -> ApiGatewayRequestAuthorizerJwtDescription {
            let mut jwt = ApiGatewayRequestAuthorizerJwtDescription::default();
            jwt.claims = claims;
            jwt
        }

        fn create_authorizer_with_jwt(
            jwt: Option<ApiGatewayRequestAuthorizerJwtDescription>,
        ) -> ApiGatewayRequestAuthorizer {
            let mut authorizer = ApiGatewayRequestAuthorizer::default();
            authorizer.jwt = jwt;
            authorizer
        }

        fn create_request_context_with_authorizer(
            authorizer: Option<ApiGatewayRequestAuthorizer>,
        ) -> ApiGatewayV2httpRequestContext {
            let mut ctx = ApiGatewayV2httpRequestContext::default();
            ctx.authorizer = authorizer;
            ctx
        }

        #[test]
        fn should_extract_user_id() {
            let expected = Uuid::new_v4();
            let claims = HashMap::from([("sub".to_string(), expected.to_string())]);
            let jwt = create_jwt_description_with_claims(claims);
            let authorizer = create_authorizer_with_jwt(Some(jwt));
            let request_context = create_request_context_with_authorizer(Some(authorizer));

            let actual = extract_user_id_request_context(&request_context).unwrap().0;

            assert_eq!(expected, actual);
        }

        #[test]
        fn should_401_when_authorizer_information_missing() {
            let request_context = ApiGatewayV2httpRequestContext::default();

            let actual = extract_user_id_request_context(&request_context).unwrap_err();

            assert_eq!(401, actual.status);
            assert_eq!("UNAUTHORIZED", actual.error.as_str());
        }

        #[test]
        fn should_401_when_jwt_missing() {
            let authorizer = create_authorizer_with_jwt(None);
            let request_context = create_request_context_with_authorizer(Some(authorizer));

            let actual = extract_user_id_request_context(&request_context).unwrap_err();

            assert_eq!(401, actual.status);
            assert_eq!("UNAUTHORIZED", actual.error.as_str());
        }

        #[test]
        fn should_500_when_claim_sub_missing() {
            let jwt = ApiGatewayRequestAuthorizerJwtDescription::default();
            let authorizer = create_authorizer_with_jwt(Some(jwt));
            let request_context = create_request_context_with_authorizer(Some(authorizer));

            let actual = extract_user_id_request_context(&request_context).unwrap_err();

            assert_eq!(500, actual.status);
            assert_eq!("INTERNAL_SERVER_ERROR", actual.error.as_str());
        }

        #[trace]
        #[rstest::rstest]
        #[case("")]
        #[case("boop")]
        #[case("foo")]
        #[case("4bf40051-84cc-4ebf-898c")]
        fn should_500_when_claim_sub_is_not_valid_uuid(#[case] sub: String) {
            let claims = HashMap::from_iter([("sub".to_string(), sub)]);
            let jwt = create_jwt_description_with_claims(claims);
            let authorizer = create_authorizer_with_jwt(Some(jwt));
            let request_context = create_request_context_with_authorizer(Some(authorizer));

            let actual = extract_user_id_request_context(&request_context).unwrap_err();

            assert_eq!(500, actual.status);
            assert_eq!("INTERNAL_SERVER_ERROR", actual.error.as_str());
        }
    }
}
