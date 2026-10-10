use super::*;
use aws_sdk_geoplaces::{
    config::{Credentials, Region, retry::RetryConfig},
    operation::geocode::GeocodeOutput,
    types::{GeocodeResultItem, MatchScoreDetails, PlaceType},
};
use geo::{
    AddressText, CountryCode, DerivedGeography, GeoPoint, GeographicDescription, SpatialPrecision,
    SubdivisionCode, SubmittedGeography,
};
use geo_service::geocoding::{
    CandidateIssue, CandidateUsability, GeocodingOutcome, GeocodingPurpose, GeocodingStorageScope,
    GeographicConstraint,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{error::Error, time::Duration};
use user_core::user_id::UserId;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn request(purpose: GeocodingPurpose) -> GeocodingRequest {
    GeocodingRequest::new(
        AddressText::new("  Vancouver\r\nBritish Columbia  ").unwrap(),
        purpose,
        5,
    )
    .unwrap()
}

fn reviewed_config() -> AmazonLocationConfig {
    AmazonLocationConfig::new(
        Duration::from_secs(6),
        Duration::from_secs(2),
        3,
        StoragePolicy::Reviewed(
            ReviewedStoragePolicy::new(
                "synthetic-offline-review".into(),
                vec![CountryCode::CAN, CountryCode::DEU, CountryCode::USA],
                true,
                true,
            )
            .unwrap(),
        ),
    )
    .unwrap()
}

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

// The public constructor pins AWS egress. Test-only composition replaces that endpoint
// with an isolated local fixture server after applying the actual timeout/retry settings.
fn provider(server: &MockServer, config: AmazonLocationConfig) -> AmazonLocationGeocoder {
    let bounded = config.bounded_client(&sdk_client(&server.uri()));
    let client = Client::from_conf(
        bounded
            .config()
            .to_builder()
            .endpoint_url(server.uri())
            .retry_config(
                RetryConfig::standard()
                    .with_max_attempts(config.max_attempts)
                    .with_initial_backoff(Duration::from_millis(1)),
            )
            .build(),
    );
    AmazonLocationGeocoder {
        client,
        config,
        in_flight: tokio::sync::Semaphore::new(MAX_IN_FLIGHT),
    }
}

fn success(body: Value, bucket: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("x-amz-geo-pricing-bucket", bucket)
        .set_body_json(body)
}

fn canada() -> Value {
    json!({"ResultItems": [{"PlaceId": "synthetic-ca", "PlaceType": "Locality",
        "Title": "Vancouver", "Position": [-123.12, 49.28],
        "Address": {"Country": {"Code2": "CA", "Code3": "CAN"},
            "Region": {"Code": "BC", "Name": "British Columbia"}}}]})
}

async fn mount(server: &MockServer, response: ResponseTemplate, count: u64) {
    Mock::given(method("POST"))
        .and(path("/v2/geocode"))
        .respond_with(response)
        .expect(count)
        .mount(server)
        .await;
}

fn candidates(result: &GeocodingResult) -> &[geo_service::geocoding::GeocodingCandidate] {
    match &result.outcome {
        GeocodingOutcome::Candidates(candidates) => candidates.as_slice(),
        GeocodingOutcome::NoMatch => panic!("expected fixture candidates"),
    }
}

#[derive(Deserialize)]
struct Fixture {
    name: String,
    query: String,
    response: Value,
    countries: Vec<Option<String>>,
    precisions: Vec<Option<String>>,
    subdivisions: Vec<Option<String>>,
    issues: Vec<String>,
}

#[tokio::test]
async fn representative_offline_fixtures_use_real_sdk_encoding_and_parsing() {
    let fixtures: Vec<Fixture> =
        serde_json::from_str(include_str!("../fixtures/geocode.json")).unwrap();
    let server = MockServer::start().await;
    let provider = provider(&server, AmazonLocationConfig::default());
    for fixture in fixtures {
        server.reset().await;
        mount(&server, success(fixture.response.clone(), "Core"), 1).await;
        let request = GeocodingRequest::new(
            AddressText::new(fixture.query.clone()).unwrap(),
            GeocodingPurpose::DealerPreview,
            5,
        )
        .unwrap();
        let result = provider.geocode(&request).await.unwrap();
        if fixture.countries.is_empty() {
            assert!(
                matches!(result.outcome, GeocodingOutcome::NoMatch),
                "{}",
                fixture.name
            );
        } else {
            let candidates = candidates(&result);
            assert_eq!(
                fixture.countries.len(),
                candidates.len(),
                "{}",
                fixture.name
            );
            for (index, candidate) in candidates.iter().enumerate() {
                let geography = candidate.geography().unwrap();
                let description = geography.description();
                assert_eq!(
                    fixture.countries[index].as_deref(),
                    description
                        .and_then(GeographicDescription::country)
                        .map(|country| country.alpha2()),
                    "{}",
                    fixture.name
                );
                assert_eq!(
                    fixture.subdivisions[index].as_deref(),
                    description
                        .and_then(GeographicDescription::subdivision)
                        .map(SubdivisionCode::as_str),
                    "{}",
                    fixture.name
                );
                assert_eq!(
                    fixture.precisions[index].as_deref(),
                    geography.position().map(|p| p.precision().as_str()),
                    "{}",
                    fixture.name
                );
                if let Some(position) = geography.position() {
                    let coordinates = &fixture.response["ResultItems"][index]["Position"];
                    assert_eq!(
                        coordinates[0].as_f64().unwrap(),
                        position.point().longitude_degrees()
                    );
                    assert_eq!(
                        coordinates[1].as_f64().unwrap(),
                        position.point().latitude_degrees()
                    );
                    assert_eq!(None, position.accuracy_metres());
                }
                assert_eq!(
                    fixture.issues,
                    candidate
                        .issues()
                        .iter()
                        .map(|issue| format!("{issue:?}"))
                        .collect::<Vec<_>>(),
                    "{}",
                    fixture.name
                );
                assert_eq!(CandidateUsability::Usable, candidate.usability());
                assert_eq!("AMAZON_LOCATION", candidate.provenance().provider);
                assert_eq!(REGION, candidate.provenance().region);
                assert_eq!(TERMS_PROFILE, candidate.provenance().interpretation_profile);
            }
            if let GeocodingOutcome::Candidates(set) = &result.outcome {
                assert_eq!(fixture.countries.len() > 1, set.has_multiple_candidates());
            }
        }
        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(fixture.query, body["QueryText"].as_str().unwrap());
        assert_eq!("SingleUse", body["IntendedUse"]);
        assert_eq!(5, body["MaxResults"]);
        assert!(body.get("Filter").is_none());
        assert!(body.get("AdditionalFeatures").is_none());
        assert_eq!(fixture.query, request.query().as_str());
        assert!(
            !result
                .usage
                .permits_storage_in(GeocodingStorageScope::DealerShared)
        );
    }
}

#[tokio::test]
async fn assertions_constraints_hints_and_language_have_distinct_semantics() {
    let server = MockServer::start().await;
    mount(&server, success(canada(), "Core"), 1).await;
    let submitted = SubmittedGeography::new(
        GeographicDescription::new(
            Some(AddressText::new("submitted observation").unwrap()),
            Some(CountryCode::DEU),
            Some(SubdivisionCode::new("DE-BE").unwrap()),
            None,
        )
        .unwrap()
        .unwrap(),
    );
    let request = request(GeocodingPurpose::DealerPreview)
        .with_submitted(submitted.clone())
        .with_constraint(GeographicConstraint::new(Some(CountryCode::USA), None).unwrap())
        .with_bias_position(GeoPoint::new(52.52, 13.405).unwrap())
        .with_language(localization::Language::Fr);
    let result = provider(&server, AmazonLocationConfig::default())
        .geocode(&request)
        .await
        .unwrap();
    let candidate = &candidates(&result)[0];
    assert_eq!(
        CandidateUsability::ConstraintMismatch,
        candidate.usability()
    );
    assert!(
        candidate
            .issues()
            .contains(&CandidateIssue::CountryConstraintMismatch)
    );
    assert!(
        candidate
            .issues()
            .contains(&CandidateIssue::SubmittedCountryMismatch)
    );
    assert!(
        candidate
            .issues()
            .contains(&CandidateIssue::SubmittedSubdivisionMismatch)
    );
    assert_eq!(Some(&submitted), request.submitted());
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(request.query().as_str(), body["QueryText"]);
    assert_eq!(json!({"IncludeCountries": ["US"]}), body["Filter"]);
    assert_eq!(json!([13.405, 52.52]), body["BiasPosition"]);
    assert_eq!("fr", body["Language"]);
    assert_eq!(
        Some(CountryCode::CAN),
        candidate
            .geography()
            .unwrap()
            .description()
            .unwrap()
            .country()
    );
}

#[tokio::test]
async fn unsupported_subdivision_cannot_verify_a_hard_constraint() {
    let server = MockServer::start().await;
    let mut response = canada();
    response["ResultItems"][0]["Address"]["Region"]["Code"] = json!("ON");
    mount(&server, success(response, "Core"), 1).await;
    let request = request(GeocodingPurpose::DealerPreview).with_constraint(
        GeographicConstraint::new(None, Some(SubdivisionCode::new("CA-ON").unwrap())).unwrap(),
    );
    let result = provider(&server, AmazonLocationConfig::default())
        .geocode(&request)
        .await
        .unwrap();
    assert_eq!(
        CandidateUsability::UnverifiedConstraint,
        candidates(&result)[0].usability()
    );
    assert!(
        candidates(&result)[0]
            .issues()
            .contains(&CandidateIssue::UnsupportedSubdivision)
    );
}

#[tokio::test]
async fn country_and_subdivision_mismatches_are_not_absence_or_outages() {
    let server = MockServer::start().await;
    mount(&server, success(canada(), "Core"), 2).await;
    let provider = provider(&server, AmazonLocationConfig::default());
    let constrained = request(GeocodingPurpose::DealerPreview).with_constraint(
        GeographicConstraint::new(None, Some(SubdivisionCode::new("CA-ON").unwrap())).unwrap(),
    );
    let result = provider.geocode(&constrained).await.unwrap();
    assert_eq!(
        CandidateUsability::ConstraintMismatch,
        candidates(&result)[0].usability()
    );
    assert!(
        candidates(&result)[0]
            .issues()
            .contains(&CandidateIssue::SubdivisionConstraintMismatch)
    );
    let asserted =
        request(GeocodingPurpose::DealerPreview).with_submitted(SubmittedGeography::new(
            GeographicDescription::new(None, Some(CountryCode::DEU), None, None)
                .unwrap()
                .unwrap(),
        ));
    let result = provider.geocode(&asserted).await.unwrap();
    assert_eq!(
        CandidateUsability::AssertionMismatch,
        candidates(&result)[0].usability()
    );
}

#[tokio::test]
async fn contradictory_country_fields_do_not_form_coherent_geography() {
    let server = MockServer::start().await;
    let mut response = canada();
    response["ResultItems"][0]["Address"]["Country"]["Code3"] = json!("USA");
    mount(&server, success(response, "Core"), 1).await;
    let result = provider(&server, AmazonLocationConfig::default())
        .geocode(&request(GeocodingPurpose::DealerPreview))
        .await
        .unwrap();
    let candidate = &candidates(&result)[0];
    assert_eq!(None, candidate.geography());
    assert_eq!(CandidateUsability::Inconsistent, candidate.usability());
}

#[tokio::test]
async fn malformed_successes_invalid_coordinates_and_excess_candidates_fail_closed() {
    let server = MockServer::start().await;
    let provider = provider(&server, AmazonLocationConfig::default());
    let mut responses = vec![json!({}), json!({"ResultItems": [{}]})];
    // Tokyo swapped lat/lon is outside latitude bounds; both-in-range swaps are checked
    // separately against exact expected fixture coordinates, not guessed heuristically.
    for position in [
        json!([35.681, 139.767]),
        json!([0, 91]),
        json!([181, 0]),
        json!([0]),
        json!([0, 0, 0]),
        json!([null, 0]),
        json!(["NaN", 0]),
        json!([0, "Infinity"]),
    ] {
        let mut response = canada();
        response["ResultItems"][0]["Position"] = position;
        responses.push(response);
    }
    let mut over_limit = canada();
    over_limit["ResultItems"] = json!(vec![canada()["ResultItems"][0].clone(); 6]);
    responses.push(over_limit);
    for response in responses {
        server.reset().await;
        mount(&server, success(response, "Core"), 1).await;
        let error = provider
            .geocode(&request(GeocodingPurpose::DealerPreview))
            .await
            .unwrap_err();
        assert_eq!(GeocodingFailure::InvalidResponse, error.kind);
        assert!(error.source().is_some());
    }
    server.reset().await;
    mount(
        &server,
        ResponseTemplate::new(200)
            .insert_header("x-amz-geo-pricing-bucket", "Core")
            .set_body_raw("{malformed-json", "application/json"),
        1,
    )
    .await;
    let error = provider
        .geocode(&request(GeocodingPurpose::DealerPreview))
        .await
        .unwrap_err();
    assert_eq!(GeocodingFailure::InvalidResponse, error.kind);
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<SdkError<GeocodeError>>()
            .is_some()
    );
    // Nonfinite f64 values can also come from SDK/custom decoding paths, independently
    // of JSON's numeric grammar. Test the adapter-to-G1 boundary directly.
    for coordinate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let item = GeocodeResultItem::builder()
            .place_id("synthetic-nonfinite")
            .place_type(PlaceType::PointAddress)
            .title("synthetic")
            .set_position(Some(vec![coordinate, 0.0]))
            .build()
            .unwrap();
        let response = GeocodeOutput::builder()
            .pricing_bucket("Core")
            .result_items(item)
            .build()
            .unwrap();
        assert_eq!(
            GeocodingFailure::InvalidResponse,
            mapping::map_response(response, &request(GeocodingPurpose::DealerPreview), None)
                .unwrap_err()
                .kind
        );
    }
}

#[test]
fn invalid_scores_are_not_calibrated_probabilities_or_silent_absence() {
    for score in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        let item = GeocodeResultItem::builder()
            .place_id("synthetic-score")
            .place_type(PlaceType::Country)
            .title("synthetic")
            .match_scores(MatchScoreDetails::builder().overall(score).build())
            .build()
            .unwrap();
        let response = GeocodeOutput::builder()
            .pricing_bucket("Core")
            .result_items(item)
            .build()
            .unwrap();
        assert_eq!(
            GeocodingFailure::InvalidResponse,
            mapping::map_response(response, &request(GeocodingPurpose::DealerPreview), None)
                .unwrap_err()
                .kind
        );
    }
}

#[tokio::test]
async fn sdk_defaulted_match_scores_do_not_invent_provider_evidence() {
    let server = MockServer::start().await;
    let provider = provider(&server, AmazonLocationConfig::default());
    for (scores, expected) in [
        (json!({}), None),
        (json!({"Components": {"Address": {"Country": 1.0}}}), None),
        (json!({"Overall": null}), None),
        (json!({"Overall": 0.0}), None),
        (json!({"Overall": 0.8}), Some(0.8)),
    ] {
        server.reset().await;
        let mut response = canada();
        response["ResultItems"][0]["MatchScores"] = scores;
        mount(&server, success(response, "Core"), 1).await;
        let result = provider
            .geocode(&request(GeocodingPurpose::DealerPreview))
            .await
            .unwrap();
        assert_eq!(expected, candidates(&result)[0].score().map(|s| s.value()));
    }
}

#[tokio::test]
async fn retained_results_encode_storage_intent_and_keep_scope_permissions() {
    let server = MockServer::start().await;
    mount(&server, success(canada(), "Stored"), 2).await;
    let provider = provider(&server, reviewed_config());
    let user = UserId::new();
    for purpose in [
        GeocodingPurpose::DealerReusable,
        GeocodingPurpose::PrivateRetained { user_id: user },
    ] {
        let request = request(purpose)
            .with_constraint(GeographicConstraint::new(Some(CountryCode::CAN), None).unwrap());
        let result = provider.geocode(&request).await.unwrap();
        assert_eq!(
            Some("synthetic-offline-review"),
            result.usage.storage_review()
        );
        assert!(
            result
                .usage
                .permits_storage_in(purpose.storage_scope().unwrap())
        );
        assert_eq!(
            purpose == GeocodingPurpose::DealerReusable,
            result
                .usage
                .permits_storage_in(GeocodingStorageScope::DealerShared)
        );
        assert!(
            !result
                .usage
                .permits_storage_in(GeocodingStorageScope::PrivateUser {
                    user_id: UserId::new()
                })
        );
    }
    for received in server.received_requests().await.unwrap() {
        let body: Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!("Storage", body["IntendedUse"]);
        assert_eq!(json!(["CA"]), body["Filter"]["IncludeCountries"]);
    }
}

#[tokio::test]
async fn storage_gates_reject_before_network_and_recheck_returned_geography() {
    let server = MockServer::start().await;
    let no_storage = provider(&server, AmazonLocationConfig::default());
    let request = request(GeocodingPurpose::DealerReusable);
    assert_eq!(
        GeocodingFailure::StorageNotPermitted,
        no_storage.geocode(&request).await.unwrap_err().kind
    );
    let provider = provider(&server, reviewed_config());
    for country in [None, Some(CountryCode::JPN), Some(CountryCode::FRA)] {
        let mut request = request.clone();
        if let Some(country) = country {
            request =
                request.with_constraint(GeographicConstraint::new(Some(country), None).unwrap());
        }
        assert_eq!(
            GeocodingFailure::StorageNotPermitted,
            provider.geocode(&request).await.unwrap_err().kind
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    let request =
        request.with_constraint(GeographicConstraint::new(Some(CountryCode::CAN), None).unwrap());
    for country in [
        json!({"Code2": "JP", "Code3": "JPN"}),
        json!({"Code2": "DE", "Code3": "DEU"}),
        json!({"Code2": "CA", "Code3": "USA"}),
        json!({"Code2": "XX"}),
        json!({"Code2": "ca", "Code3": "CAN"}),
        json!({}),
    ] {
        server.reset().await;
        let mut response = canada();
        response["ResultItems"][0]["Address"]["Country"] = country;
        mount(&server, success(response, "Stored"), 1).await;
        assert_eq!(
            GeocodingFailure::StorageNotPermitted,
            provider.geocode(&request).await.unwrap_err().kind
        );
    }
}

#[tokio::test]
async fn outages_throttling_authentication_and_bad_configuration_preserve_causes() {
    let server = MockServer::start().await;
    let provider = provider(&server, AmazonLocationConfig::default());
    for (status, code, expected, attempts) in [
        (429, "ThrottlingException", GeocodingFailure::Throttled, 3),
        (
            503,
            "InternalServerException",
            GeocodingFailure::Unavailable,
            3,
        ),
        (
            403,
            "AccessDeniedException",
            GeocodingFailure::Authentication,
            1,
        ),
        (
            400,
            "ValidationException",
            GeocodingFailure::Configuration,
            1,
        ),
    ] {
        server.reset().await;
        mount(
            &server,
            ResponseTemplate::new(status).set_body_json(json!({"__type": code,
            "Message": "sensitive provider address and payload"})),
            attempts,
        )
        .await;
        let error = provider
            .geocode(&request(GeocodingPurpose::DealerPreview))
            .await
            .unwrap_err();
        assert_eq!(expected, error.kind);
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<SdkError<GeocodeError>>()
                .is_some()
        );
        assert!(!format!("{error:?} {error}").contains("sensitive"));
        assert_eq!(
            attempts as usize,
            server.received_requests().await.unwrap().len()
        );
    }
}

#[tokio::test]
async fn attempt_timeout_and_total_deadline_are_bounded() {
    let server = MockServer::start().await;
    mount(
        &server,
        success(canada(), "Core").set_delay(Duration::from_secs(2)),
        1,
    )
    .await;
    let config = AmazonLocationConfig::new(
        Duration::from_secs(1),
        Duration::from_millis(30),
        1,
        StoragePolicy::Disabled,
    )
    .unwrap();
    let provider = provider(&server, config);
    let start = tokio::time::Instant::now();
    let error = provider
        .geocode(&request(GeocodingPurpose::DealerPreview))
        .await
        .unwrap_err();
    assert_eq!(GeocodingFailure::Timeout, error.kind);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(error.source().is_some());
    server.reset().await;
    mount(
        &server,
        success(canada(), "Core").set_delay(Duration::from_secs(2)),
        1,
    )
    .await;
    let provider = super::tests::provider(
        &server,
        AmazonLocationConfig::new(
            Duration::from_secs(1),
            Duration::from_secs(1),
            3,
            StoragePolicy::Disabled,
        )
        .unwrap(),
    );
    let start = tokio::time::Instant::now();
    let error = provider
        .geocode(&request(GeocodingPurpose::DealerPreview))
        .await
        .unwrap_err();
    assert_eq!(GeocodingFailure::Timeout, error.kind);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn unparseable_error_responses_preserve_status_classification_and_cause() {
    for (status, expected) in [
        (200_u16, GeocodingFailure::InvalidResponse),
        (400, GeocodingFailure::Configuration),
        (401, GeocodingFailure::Authentication),
        (403, GeocodingFailure::Authentication),
        (408, GeocodingFailure::Timeout),
        (429, GeocodingFailure::Throttled),
        (502, GeocodingFailure::Unavailable),
        (504, GeocodingFailure::Timeout),
    ] {
        let source = SdkError::response_error(
            static_error("sensitive provider body"),
            aws_sdk_geoplaces::config::http::HttpResponse::new(
                status.try_into().unwrap(),
                "sensitive provider body".into(),
            ),
        );
        let error = map_sdk_error(source);
        assert_eq!(expected, error.kind);
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<SdkError<GeocodeError>>()
                .is_some()
        );
        assert!(!format!("{error:?} {error}").contains("sensitive"));
    }
}

#[tokio::test(start_paused = true)]
async fn concurrency_queue_is_included_in_total_deadline() {
    let server = MockServer::start().await;
    let provider = provider(&server, AmazonLocationConfig::default());
    let _permits = provider
        .in_flight
        .acquire_many(MAX_IN_FLIGHT as u32)
        .await
        .unwrap();
    let error = provider
        .geocode(&request(GeocodingPurpose::DealerPreview))
        .await
        .unwrap_err();
    assert_eq!(GeocodingFailure::Timeout, error.kind);
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<tokio::time::error::Elapsed>()
            .is_some()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn provider_text_bound_counts_unicode_characters_without_truncation() {
    let server = MockServer::start().await;
    let provider = provider(&server, AmazonLocationConfig::default());
    let too_long = GeocodingRequest::new(
        AddressText::new("東".repeat(201)).unwrap(),
        GeocodingPurpose::DealerPreview,
        5,
    )
    .unwrap();
    assert_eq!(
        GeocodingFailure::UnsupportedInput,
        provider.geocode(&too_long).await.unwrap_err().kind
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    mount(&server, success(json!({"ResultItems": []}), "Core"), 1).await;
    let accepted = GeocodingRequest::new(
        AddressText::new("東".repeat(200)).unwrap(),
        GeocodingPurpose::DealerPreview,
        5,
    )
    .unwrap();
    assert!(matches!(
        provider.geocode(&accepted).await.unwrap().outcome,
        GeocodingOutcome::NoMatch
    ));
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(accepted.query().as_str(), body["QueryText"]);
}

#[tokio::test]
async fn reviewed_public_and_private_purposes_are_independently_enabled() {
    let server = MockServer::start().await;
    for (dealer, private, denied_purpose) in [
        (
            true,
            false,
            GeocodingPurpose::PrivateRetained {
                user_id: UserId::new(),
            },
        ),
        (false, true, GeocodingPurpose::DealerReusable),
    ] {
        let config = AmazonLocationConfig::new(
            Duration::from_secs(6),
            Duration::from_secs(2),
            3,
            StoragePolicy::Reviewed(
                ReviewedStoragePolicy::new(
                    "review".into(),
                    vec![CountryCode::CAN],
                    dealer,
                    private,
                )
                .unwrap(),
            ),
        )
        .unwrap();
        let provider = provider(&server, config);
        let request = request(denied_purpose)
            .with_constraint(GeographicConstraint::new(Some(CountryCode::CAN), None).unwrap());
        assert_eq!(
            GeocodingFailure::StorageNotPermitted,
            provider.geocode(&request).await.unwrap_err().kind
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn configuration_is_validated_and_inherited_request_bounds_are_overridden() {
    for (deadline, attempt, tries) in [
        (0, 1, 1),
        (31, 1, 1),
        (1, 0, 1),
        (1, 2, 1),
        (1, 1, 0),
        (1, 1, 4),
    ] {
        assert!(
            AmazonLocationConfig::new(
                Duration::from_secs(deadline),
                Duration::from_secs(attempt),
                tries,
                StoragePolicy::Disabled
            )
            .is_err()
        );
    }
    assert!(
        ReviewedStoragePolicy::new("review".into(), vec![CountryCode::JPN], true, false).is_err()
    );
    assert!(ReviewedStoragePolicy::new(" ".into(), vec![CountryCode::DEU], true, false).is_err());
    assert!(ReviewedStoragePolicy::new("review".into(), vec![], true, false).is_err());
    assert!(
        ReviewedStoragePolicy::new("review".into(), vec![CountryCode::DEU], false, false).is_err()
    );
    let config = AmazonLocationConfig::default();
    let client = sdk_client(ENDPOINT);
    let client = Client::from_conf(
        client
            .config()
            .to_builder()
            .retry_config(RetryConfig::standard().with_max_attempts(100))
            .timeout_config(aws_sdk_geoplaces::config::timeout::TimeoutConfig::disabled())
            .build(),
    );
    let provider = AmazonLocationGeocoder::new(client, config).unwrap();
    assert_eq!(
        3,
        provider
            .client
            .config()
            .retry_config()
            .unwrap()
            .max_attempts()
    );
    let timeouts = provider.client.config().timeout_config().unwrap();
    assert_eq!(Some(Duration::from_secs(6)), timeouts.operation_timeout());
    assert_eq!(
        Some(Duration::from_secs(2)),
        timeouts.operation_attempt_timeout()
    );
    let wrong_region = Client::from_conf(
        sdk_client(ENDPOINT)
            .config()
            .to_builder()
            .region(Region::new("ap-southeast-1"))
            .build(),
    );
    assert!(AmazonLocationGeocoder::new(wrong_region, AmazonLocationConfig::default()).is_err());
}

#[test]
fn zero_zero_is_valid_and_unknown_precision_does_not_become_domain_precision() {
    let item = GeocodeResultItem::builder()
        .place_id("synthetic-zero")
        .place_type(PlaceType::PointAddress)
        .title("synthetic")
        .set_position(Some(vec![0.0, 0.0]))
        .build()
        .unwrap();
    let response = GeocodeOutput::builder()
        .pricing_bucket("Core")
        .result_items(item)
        .build()
        .unwrap();
    let result =
        mapping::map_response(response, &request(GeocodingPurpose::DealerPreview), None).unwrap();
    let position = candidates(&result)[0]
        .geography()
        .and_then(DerivedGeography::position)
        .unwrap();
    assert_eq!(GeoPoint::new(0.0, 0.0).unwrap(), position.point());
    assert_eq!(SpatialPrecision::Premises, position.precision());
}
