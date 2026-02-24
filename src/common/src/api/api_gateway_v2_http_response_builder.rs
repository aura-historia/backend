use crate::api::error::ApiError;
use crate::api::error_code::INTERNAL_SERVER_ERROR;
use crate::language::data::LanguageData;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequestContext, ApiGatewayV2httpResponse};
use aws_lambda_events::encodings::Body;
use http::header::{CONTENT_LANGUAGE, CONTENT_TYPE, ETAG, LAST_MODIFIED, LOCATION};
use http::{HeaderMap, HeaderName, HeaderValue};
use httpdate::fmt_http_date;
use serde::Serialize;
use std::fmt::Debug;
use std::time::SystemTime;
use tracing::error;

#[derive(Debug, Clone, PartialEq)]
pub struct ApiGatewayV2HttpResponseBuilder {
    status_code: i64,
    headers: HeaderMap,
    body: Option<Body>,
    is_base64_encoded: bool,
}

impl ApiGatewayV2HttpResponseBuilder {
    pub fn new(status_code: i64) -> Self {
        Self {
            status_code,
            headers: HeaderMap::new(),
            body: None,
            is_base64_encoded: false,
        }
    }

    pub fn json(status_code: i64) -> Self {
        Self::new(status_code).content_type("application/json")
    }

    pub fn plain(status_code: i64) -> Self {
        Self::new(status_code).content_type("text/plain")
    }

    pub fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
        self
    }

    pub fn content_type(mut self, content_type: &'static str) -> Self {
        self.headers
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        self
    }

    pub fn content_language(mut self, language: LanguageData) -> Self {
        match serde_json::to_value(language) {
            Ok(content_language) => match content_language.as_str() {
                None => {
                    error!(
                        language = ?language,
                        type = %std::any::type_name::<LanguageData>(),
                        "Failed to serialize LanguageData as JSON-Value-String when setting HTTP Content-Language."
                    );
                }
                Some(content_language_str) => match HeaderValue::from_str(content_language_str) {
                    Ok(header_value) => {
                        self.headers.insert(CONTENT_LANGUAGE, header_value);
                    }
                    Err(err) => {
                        error!(
                            error = %err,
                            language = ?content_language,
                            "Failed to convert serialized LanguageData to HeaderValue when setting HTTP Content-Language."
                        );
                    }
                },
            },
            Err(err) => {
                error!(
                    error = %err,
                    language = ?language,
                    type = %std::any::type_name::<LanguageData>(),
                    "Failed to serialize LanguageData when setting HTTP Content-Language."
                );
            }
        }
        self
    }

    pub fn try_content_language(self, content_language_opt: Option<LanguageData>) -> Self {
        if let Some(content_language) = content_language_opt {
            self.content_language(content_language)
        } else {
            self
        }
    }

    pub fn e_tag(mut self, e_tag: &str) -> Self {
        match HeaderValue::from_str(e_tag) {
            Ok(e_tag_value) => {
                self.headers.insert(ETAG, e_tag_value);
            }
            Err(err) => {
                error!(
                    error = %err,
                    eTag = %e_tag,
                    "Failed to convert e_tag to HeaderValue when setting HTTP ETag."
                )
            }
        }
        self
    }

    pub fn last_modified(mut self, last_modified_time: impl Into<SystemTime>) -> Self {
        let last_modified = fmt_http_date(last_modified_time.into());
        match HeaderValue::from_str(&last_modified) {
            Ok(last_modified_value) => {
                self.headers.insert(LAST_MODIFIED, last_modified_value);
            }
            Err(err) => {
                error!(
                    error = %err,
                    lastModified = %last_modified,
                    "Failed to convert lastModified to HeaderValue when setting HTTP Last-Modified."
                )
            }
        }
        self
    }

    pub fn location(
        mut self,
        path: &str,
        request_context: &ApiGatewayV2httpRequestContext,
    ) -> Self {
        let host = match request_context.stage.as_deref() {
            Some("prod") => "api.aura-historia.com",
            Some("dev") => "api.dev.aura-historia.com",
            _ => request_context
                .domain_name
                .as_deref()
                .unwrap_or("api.dev.aura-historia.com"),
        };
        let location = format!("https://{host}/api/v1/{path}");
        match HeaderValue::from_str(&location) {
            Ok(location_value) => {
                self.headers.insert(LOCATION, location_value);
            }
            Err(err) => {
                error!(
                    error = %err,
                    path = path,
                    location = location,
                    "Failed to convert location to HeaderValue when setting HTTP Location."
                )
            }
        }
        self
    }

    pub fn cache_control(
        mut self,
        directive: &'static str,
        max_age: Option<u64>,
        s_max_age: Option<u64>,
    ) -> Self {
        let mut parts = vec![directive.to_string()];

        if let Some(age) = max_age {
            parts.push(format!("max-age={age}"));
        }

        if let Some(age) = s_max_age {
            parts.push(format!("s-maxage={age}"));
        }

        let val = parts.join(", ");

        self.headers.insert(
            http::header::CACHE_CONTROL,
            HeaderValue::from_str(&val).expect("invalid Cache-Control header value"),
        );

        self
    }

    pub fn body<T: Into<Body>>(mut self, body: T) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn body_serde<T: Serialize + Debug>(mut self, t: T) -> Result<Self, ApiError> {
        let body = serde_json::to_string(&t).map_err(|err| {
            tracing::error!(error = %err, payload = ?t, type = %std::any::type_name::<T>(), "Failed serializing.");
            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
        })?;
        self.body = Some(body.into());

        Ok(self)
    }

    pub fn base64_encoded(mut self, flag: bool) -> Self {
        self.is_base64_encoded = flag;
        self
    }

    pub fn build(self) -> ApiGatewayV2httpResponse {
        let mut response = ApiGatewayV2httpResponse::default();
        response.status_code = self.status_code;
        response.headers = self.headers;
        response.body = self.body;
        response.is_base64_encoded = self.is_base64_encoded;
        response
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        language::data::LanguageData,
    };
    use aws_lambda_events::apigw::ApiGatewayV2httpRequestContext;
    use http::header::LOCATION;
    use rstest;
    use std::time::SystemTime;

    #[rstest::rstest]
    #[case::minimal_100(ApiGatewayV2HttpResponseBuilder::new(100))]
    #[case::minimal_200(ApiGatewayV2HttpResponseBuilder::new(200))]
    #[case::minimal_300(ApiGatewayV2HttpResponseBuilder::new(300))]
    #[case::minimal_400(ApiGatewayV2HttpResponseBuilder::new(400))]
    #[case::minimal_500(ApiGatewayV2HttpResponseBuilder::new(500))]
    #[case::json(ApiGatewayV2HttpResponseBuilder::json(200))]
    #[case::plain_text(ApiGatewayV2HttpResponseBuilder::plain(200))]
    #[case::content_language(ApiGatewayV2HttpResponseBuilder::new(200).content_language(LanguageData::De))]
    #[case::try_content_language(ApiGatewayV2HttpResponseBuilder::new(200).try_content_language(Some(LanguageData::En)))]
    #[case::e_tag(ApiGatewayV2HttpResponseBuilder::new(200).e_tag("123456"))]
    #[case::last_modified(ApiGatewayV2HttpResponseBuilder::new(200).last_modified(SystemTime::now()))]
    #[trace]
    fn should_build_api_gateway_proxy_response(#[case] builder: ApiGatewayV2HttpResponseBuilder) {
        let _ = builder.build();
    }

    #[rstest::rstest]
    #[case(
        "prod",
        "shops/foo/products/bar",
        "https://api.aura-historia.com/api/v1/shops/foo/products/bar"
    )]
    #[case(
        "dev",
        "shops/foo/products/bar",
        "https://api.dev.aura-historia.com/api/v1/shops/foo/products/bar"
    )]
    #[case(
        "prod",
        "watchlist/foo/bar",
        "https://api.aura-historia.com/api/v1/watchlist/foo/bar"
    )]
    #[case(
        "dev",
        "watchlist/foo/bar",
        "https://api.dev.aura-historia.com/api/v1/watchlist/foo/bar"
    )]
    #[trace]
    fn should_build_location_correctly(
        #[case] stage: String,
        #[case] path: String,
        #[case] expected_location: String,
    ) {
        let mut request_context = ApiGatewayV2httpRequestContext::default();
        request_context.stage = Some(stage);

        let response = ApiGatewayV2HttpResponseBuilder::new(200)
            .location(&path, &request_context)
            .build();
        let actual = response.headers.get(LOCATION).unwrap().to_str().unwrap();

        assert_eq!(&expected_location, actual);
    }

    #[rstest::rstest]
    #[case("public", Some(60), Some(900), "public, max-age=60, s-maxage=900")]
    #[case("public", Some(120), None, "public, max-age=120")]
    #[case("public", None, Some(300), "public, s-maxage=300")]
    #[case("no-store", None, None, "no-store")]
    #[trace]
    fn should_build_cache_control_correctly(
        #[case] directive: &'static str,
        #[case] max_age: Option<u64>,
        #[case] s_max_age: Option<u64>,
        #[case] expected: &'static str,
    ) {
        let response = ApiGatewayV2HttpResponseBuilder::new(200)
            .cache_control(directive, max_age, s_max_age)
            .build();

        let actual = response
            .headers
            .get(http::header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap();

        assert_eq!(expected, actual);
    }
}
