use super::*;
#[test]
fn envelope_preserves_duplicate_fields_and_rejects_late_syntax_errors() {
    let items = parse_envelope("[{\"a\":1,\"a\":2},null]").unwrap();
    assert_eq!(items[0].get(), "{\"a\":1,\"a\":2}");
    assert_eq!(items[1].get(), "null");
    for invalid in ["", "  ", "{}", "[{},", "[{},]", "[{},{}]garbage"] {
        assert!(parse_envelope(invalid).is_err(), "{invalid}");
    }
    assert!(parse_envelope(&format!("[{}]", vec!["null"; 101].join(","))).is_err());
}
#[test]
fn status_precedence() {
    let failure = |kind, retryable| BatchFailure {
        index: 0,
        source_listing_id: None,
        error: "SAFE",
        retryable,
        kind,
    };
    assert_eq!(status_for(0, &[]), StatusCode::ACCEPTED);
    assert_eq!(
        status_for(1, &[failure(FailureKind::Send, true)]),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        status_for(0, &[failure(FailureKind::Invalid, false)]),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for(0, &[failure(FailureKind::MessageSize, false)]),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        status_for(
            0,
            &[
                failure(FailureKind::MessageSize, false),
                failure(FailureKind::Invalid, false)
            ]
        ),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_for(0, &[failure(FailureKind::Internal, false)]),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        status_for(
            0,
            &[
                failure(FailureKind::Internal, false),
                failure(FailureKind::Send, true)
            ]
        ),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn key_rejects_duplicate_headers_and_invalid_values() {
    let mut headers = HeaderMap::new();
    headers.append(IDEMPOTENCY_KEY, HeaderValue::from_static("first"));
    headers.append(IDEMPOTENCY_KEY, HeaderValue::from_static("second"));
    assert!(idempotency_key(&headers).is_err());
    headers.clear();
    for invalid in ["contains space", "first,second", "first, second", ","] {
        headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_str(invalid).unwrap());
        let error = idempotency_key(&headers).unwrap_err();
        assert_eq!(error.code(), BAD_HEADER_VALUE, "{invalid}");
    }
    headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_static("okay"));
    assert_eq!(idempotency_key(&headers).unwrap().unwrap().as_str(), "okay");
}

#[test]
fn http_key_grammar_covers_visible_ascii_except_comma() {
    let mut headers = HeaderMap::new();
    for byte in 0x21..=0x7e {
        let value = HeaderValue::from_bytes(&[byte]).unwrap();
        headers.insert(IDEMPOTENCY_KEY, value);
        assert_eq!(
            idempotency_key(&headers).is_ok(),
            byte != b',',
            "byte {byte}"
        );
    }
    headers.insert(IDEMPOTENCY_KEY, HeaderValue::from_static(""));
    assert!(idempotency_key(&headers).is_err());
    headers.insert(
        IDEMPOTENCY_KEY,
        HeaderValue::from_str(&"a".repeat(128)).unwrap(),
    );
    assert!(idempotency_key(&headers).is_ok());
    headers.insert(
        IDEMPOTENCY_KEY,
        HeaderValue::from_str(&"a".repeat(129)).unwrap(),
    );
    assert!(idempotency_key(&headers).is_err());
}

#[test]
fn publisher_outcomes_have_bounded_codes_and_correct_status() {
    use product_listing_service::use_cases::{
        ProductListingIngestionCommandId, ProductListingIngestionOutcome as Outcome,
    };
    let command_id =
        ProductListingIngestionCommandId::from_wire(&format!("plic1_{}", "a".repeat(64))).unwrap();
    let make = |outcome| ProductListingIngestionItemOutcome {
        index: 1,
        command_id: command_id.clone(),
        outcome,
    };
    let cases = [
        (
            Outcome::Unconfirmed,
            "ENQUEUE_UNCONFIRMED",
            true,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            Outcome::NotAttempted {
                reason: ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                retryable: true,
            },
            "ENQUEUE_NOT_ATTEMPTED",
            true,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            Outcome::NotAttempted {
                reason: ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                retryable: false,
            },
            "ENQUEUE_BLOCKED",
            false,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            Outcome::Rejected {
                reason: ProductListingIngestionRejectionReason::Publisher {
                    code: "INVALID_MESSAGE_SIZE".into(),
                },
                retryable: false,
            },
            "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE",
            false,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            Outcome::Rejected {
                reason: ProductListingIngestionRejectionReason::Publisher {
                    code: "SQS_SENDER_FAILURE".into(),
                },
                retryable: false,
            },
            "ENQUEUE_FAILED",
            false,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            Outcome::Rejected {
                reason: ProductListingIngestionRejectionReason::Publisher {
                    code: "SQS_RETRYABLE_FAILURE".into(),
                },
                retryable: true,
            },
            "ENQUEUE_FAILED",
            true,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            Outcome::Rejected {
                reason: ProductListingIngestionRejectionReason::Publisher {
                    code: "INGESTION_INTERNAL_ERROR".into(),
                },
                retryable: false,
            },
            "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR",
            false,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];
    for (outcome, code, retryable, status) in cases {
        let failure = outcome_failure(&make(outcome), None).unwrap();
        assert_eq!(
            (failure.index, failure.error, failure.retryable),
            (1, code, retryable)
        );
        assert_eq!(status_for(0, &[failure]), status);
    }
}
