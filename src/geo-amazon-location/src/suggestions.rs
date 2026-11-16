//! Places v2 Autocomplete for address suggestions. Requests use the Label pricing bucket (no
//! additional features) and `SingleUse`, the only intended use Autocomplete supports: results
//! are shown once and never stored or cached.
use crate::{
    InvalidAmazonLocationConfig, REGION, config::AmazonLocationConfig, map_sdk_error,
    mapping::invalid_response,
};
use application::error::box_error;
use aws_sdk_geoplaces::{
    Client,
    operation::autocomplete::AutocompleteOutput,
    types::{AutocompleteIntendedUse, AutocompleteResultItem, PlaceType},
};
use geo_service::{
    geocoding::{GeocodingError, GeocodingFailure, PlaceReference},
    suggestions::{
        AddressSuggestion, AddressSuggestionProvider, AddressSuggestions, MAX_SUGGESTIONS,
        SuggestionAttribution, SuggestionKind, SuggestionQuery,
    },
};
use localization::Language;

const ATTRIBUTION: SuggestionAttribution = SuggestionAttribution {
    provider: "AMAZON_LOCATION",
    attribution_url: "https://docs.aws.amazon.com/location/latest/developerguide/data-attribution.html",
};
const MAX_IN_FLIGHT: usize = 8;
const MAX_LABEL_BYTES: usize = 1024;

pub struct AmazonLocationAddressSuggester {
    client: Client,
    config: AmazonLocationConfig,
    in_flight: tokio::sync::Semaphore,
}

impl AmazonLocationAddressSuggester {
    pub fn new(
        client: Client,
        config: AmazonLocationConfig,
    ) -> Result<Self, InvalidAmazonLocationConfig> {
        if client.config().region().map(|r| r.as_ref()) != Some(REGION) {
            return Err(InvalidAmazonLocationConfig::RegionOrEndpoint);
        }
        Ok(Self {
            client: config.bounded_client(&client),
            config,
            in_flight: tokio::sync::Semaphore::new(MAX_IN_FLIGHT),
        })
    }

    async fn autocomplete(
        &self,
        query: &SuggestionQuery,
        language: Language,
    ) -> Result<AutocompleteOutput, GeocodingError> {
        let _permit = self
            .in_flight
            .acquire()
            .await
            .map_err(|source| GeocodingError {
                kind: GeocodingFailure::Configuration,
                source: box_error(source),
            })?;
        self.client
            .autocomplete()
            .query_text(query.as_str())
            .max_results(MAX_SUGGESTIONS as i32)
            .language(language.as_str())
            .intended_use(AutocompleteIntendedUse::SingleUse)
            .send()
            .await
            .map_err(map_sdk_error)
    }
}

#[async_trait::async_trait]
impl AddressSuggestionProvider for AmazonLocationAddressSuggester {
    async fn suggest(
        &self,
        query: &SuggestionQuery,
        language: Language,
    ) -> Result<AddressSuggestions, GeocodingError> {
        let response =
            tokio::time::timeout(self.config.deadline, self.autocomplete(query, language))
                .await
                .map_err(|source| GeocodingError {
                    kind: GeocodingFailure::Timeout,
                    source: box_error(source),
                })??;
        map_response(response)
    }
}

fn map_response(response: AutocompleteOutput) -> Result<AddressSuggestions, GeocodingError> {
    if response.pricing_bucket.trim().is_empty() {
        return Err(invalid_response(
            "autocomplete response omitted its pricing metadata",
        ));
    }
    let items = response.result_items.unwrap_or_default();
    if items.len() > MAX_SUGGESTIONS {
        return Err(invalid_response(
            "provider exceeded requested suggestion limit",
        ));
    }
    Ok(AddressSuggestions {
        suggestions: items.iter().map(map_suggestion).collect::<Result<_, _>>()?,
        attribution: ATTRIBUTION,
    })
}

fn map_suggestion(item: &AutocompleteResultItem) -> Result<AddressSuggestion, GeocodingError> {
    let place = PlaceReference::new(item.place_id.as_str())
        .map_err(|_| invalid_response("invalid provider place reference"))?;
    let label = item
        .address
        .as_ref()
        .and_then(|a| a.label.as_deref())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(&item.title)
        .trim();
    if label.is_empty() || label.len() > MAX_LABEL_BYTES || label.chars().any(char::is_control) {
        return Err(invalid_response("invalid provider suggestion label"));
    }
    Ok(AddressSuggestion {
        place,
        label: label.to_owned(),
        kind: kind(&item.place_type),
    })
}

/// Unknown and future place types are still selectable suggestions; only their kind is unknown.
fn kind(place_type: &PlaceType) -> SuggestionKind {
    match place_type.as_str() {
        "PointAddress"
        | "InterpolatedAddress"
        | "SecondaryAddress"
        | "InferredSecondaryAddress" => SuggestionKind::Address,
        "Street" | "Intersection" => SuggestionKind::Street,
        "Locality" | "District" | "SubDistrict" => SuggestionKind::Locality,
        "PostalCode" => SuggestionKind::PostalCode,
        "Country" | "Region" | "SubRegion" => SuggestionKind::Region,
        _ => SuggestionKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_geoplaces::config::{Credentials, Region};
    use serde_json::{Value, json};
    use std::time::Duration;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn sdk_client(endpoint: &str) -> Client {
        Client::from_conf(
            aws_sdk_geoplaces::Config::builder()
                .behavior_version_latest()
                .region(Region::new(REGION))
                .credentials_provider(Credentials::new(
                    "test",
                    "test",
                    None,
                    None,
                    "offline-fixture",
                ))
                .endpoint_url(endpoint)
                .build(),
        )
    }

    /// The production client pins the reviewed endpoint; tests apply the real bounds, then
    /// point the client at a local fixture server.
    fn suggester(server: &MockServer) -> AmazonLocationAddressSuggester {
        let config =
            AmazonLocationConfig::new(Duration::from_secs(1), Duration::from_secs(1), 1).unwrap();
        let bounded = config.bounded_client(&sdk_client(&server.uri()));
        AmazonLocationAddressSuggester {
            client: Client::from_conf(
                bounded
                    .config()
                    .to_builder()
                    .endpoint_url(server.uri())
                    .build(),
            ),
            config,
            in_flight: tokio::sync::Semaphore::new(MAX_IN_FLIGHT),
        }
    }

    fn item(place_id: &str, place_type: &str, label: Option<&str>) -> Value {
        let mut item = json!({"PlaceId": place_id, "PlaceType": place_type, "Title": "Title only"});
        if let Some(label) = label {
            item["Address"] = json!({"Label": label});
        }
        item
    }

    async fn respond(status: u16, body: Value) -> (MockServer, AmazonLocationAddressSuggester) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/autocomplete"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("x-amz-geo-pricing-bucket", "Label")
                    .set_body_json(body),
            )
            .mount(&server)
            .await;
        let suggester = suggester(&server);
        (server, suggester)
    }

    fn query() -> SuggestionQuery {
        SuggestionQuery::new("Synthetic Street 1").unwrap()
    }

    #[tokio::test]
    async fn should_request_display_only_label_suggestions_when_suggesting() {
        let (server, suggester) = respond(
            200,
            json!({"ResultItems": [
                item("place-address", "PointAddress", Some("Synthetic Street 1, 10115 Berlin")),
                item("place-street", "Street", None),
                item("place-future", "SomethingNew", Some("Elsewhere")),
            ]}),
        )
        .await;
        let result = suggester.suggest(&query(), Language::De).await.unwrap();
        let summary = result
            .suggestions
            .iter()
            .map(|s| (s.place.as_str(), s.label.as_str(), s.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            vec![
                (
                    "place-address",
                    "Synthetic Street 1, 10115 Berlin",
                    SuggestionKind::Address
                ),
                ("place-street", "Title only", SuggestionKind::Street),
                ("place-future", "Elsewhere", SuggestionKind::Other),
            ],
            summary
        );
        assert_eq!(ATTRIBUTION, result.attribution);
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            json!({
                "QueryText": "Synthetic Street 1",
                "MaxResults": 5,
                "Language": "de",
                "IntendedUse": "SingleUse"
            }),
            body
        );
    }

    #[tokio::test]
    async fn should_reject_malformed_or_oversized_results_when_suggesting() {
        for items in [
            vec![item("", "PointAddress", Some("Label"))],
            vec![item("has space", "PointAddress", Some("Label"))],
            vec![json!({"PlaceId": "p", "PlaceType": "Street", "Title": " "})],
            (0..=MAX_SUGGESTIONS)
                .map(|i| item(&format!("p{i}"), "Street", Some("Label")))
                .collect(),
        ] {
            let (_server, suggester) = respond(200, json!({ "ResultItems": items })).await;
            assert_eq!(
                GeocodingFailure::InvalidResponse,
                suggester
                    .suggest(&query(), Language::En)
                    .await
                    .unwrap_err()
                    .kind
            );
        }
    }

    #[tokio::test]
    async fn should_classify_provider_failures_when_suggesting() {
        for (status, expected) in [
            (429, GeocodingFailure::Throttled),
            (500, GeocodingFailure::Unavailable),
            (403, GeocodingFailure::Authentication),
        ] {
            let (_server, suggester) = respond(status, json!({"Message": "synthetic"})).await;
            assert_eq!(
                expected,
                suggester
                    .suggest(&query(), Language::En)
                    .await
                    .unwrap_err()
                    .kind
            );
        }
    }
}
