use base64::{Engine as _, engine::general_purpose::STANDARD};
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serde_json::Value;
use user_service::ports::{
    IgnoredNewsletterWebhookEvent, NewsletterWebhookDeliveryId, NewsletterWebhookEmailAddress,
    NewsletterWebhookEventKind, NewsletterWebhookEventName, NewsletterWebhookHeader,
    NewsletterWebhookMailingListId, NewsletterWebhookProviderContactId,
    NewsletterWebhookVerification, NewsletterWebhookVerificationError,
    NewsletterWebhookVerificationRequest, NewsletterWebhookVerifier,
    VerifiedNewsletterWebhookEvent,
};

const SIGNATURE_TOLERANCE_SECONDS: i128 = 300;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 16;
const MAX_HEADER_NAME_BYTES: usize = 128;
const MAX_HEADER_VALUE_BYTES: usize = 8 * 1024;
const MAX_SIGNATURE_CANDIDATES: usize = 8;
const MAX_SIGNING_SECRET_BYTES: usize = 4 * 1024;
const SIGNATURE_BYTES: usize = 32;
const SUPPORTED_SCHEMA: &str = "1.0.0";

#[derive(Default)]
struct SelectedHeaders<'a> {
    id: Option<&'a [u8]>,
    timestamp: Option<&'a [u8]>,
    signature: Option<&'a [u8]>,
}

pub struct LoopsNewsletterWebhookVerifier;

impl NewsletterWebhookVerifier for LoopsNewsletterWebhookVerifier {
    fn verify(
        &self,
        request: NewsletterWebhookVerificationRequest,
    ) -> Result<NewsletterWebhookVerification, NewsletterWebhookVerificationError> {
        if request.raw_body.len() > MAX_BODY_BYTES {
            return Err(NewsletterWebhookVerificationError::BodyTooLarge);
        }

        let selected = select_headers(&request.headers)?;
        let id = selected
            .id
            .ok_or(NewsletterWebhookVerificationError::MissingRequiredHeader)?;
        let timestamp = selected
            .timestamp
            .ok_or(NewsletterWebhookVerificationError::MissingRequiredHeader)?;
        let signature = selected
            .signature
            .ok_or(NewsletterWebhookVerificationError::MissingRequiredHeader)?;

        validate_delivery_id_header(id)?;
        let timestamp_text = validate_timestamp_header(timestamp)?;
        let delivery_timestamp = timestamp_text
            .parse::<i64>()
            .map_err(|_| NewsletterWebhookVerificationError::InvalidTimestamp)?;
        let age = i128::from(request.arrived_at_unix_seconds) - i128::from(delivery_timestamp);
        if age.abs() > SIGNATURE_TOLERANCE_SECONDS {
            return Err(NewsletterWebhookVerificationError::InvalidTimestamp);
        }

        let secret = decode_signing_secret(&request.signing_secret)?;
        let candidates = parse_signature_candidates(signature)?;
        let signed_content = signed_content(id, timestamp_text.as_bytes(), &request.raw_body);
        let expected = hmac_sha256(&secret, &signed_content)?;
        let verified = candidates.iter().fold(false, |matched, candidate| {
            // Do every supported candidate comparison, even after one matches.
            openssl::memcmp::eq(&expected, candidate) | matched
        });
        if !verified {
            return Err(NewsletterWebhookVerificationError::InvalidSignature);
        }

        let payload: Value = serde_json::from_slice(&request.raw_body)
            .map_err(|_| NewsletterWebhookVerificationError::MalformedPayload)?;
        let payload = payload
            .as_object()
            .ok_or(NewsletterWebhookVerificationError::MalformedPayload)?;
        let schema = required_string(payload, "webhookSchemaVersion")?;
        if schema != SUPPORTED_SCHEMA {
            return Err(NewsletterWebhookVerificationError::UnsupportedSchema);
        }

        let provider_event_name =
            NewsletterWebhookEventName::new(required_string(payload, "eventName")?.to_owned())
                .ok_or(NewsletterWebhookVerificationError::InvalidEvent)?;
        let event_time_unix_seconds = payload
            .get("eventTime")
            .and_then(Value::as_i64)
            .ok_or(NewsletterWebhookVerificationError::InvalidEvent)?;
        let delivery_id = NewsletterWebhookDeliveryId::new(
            std::str::from_utf8(id)
                .map_err(|_| NewsletterWebhookVerificationError::InvalidHeaderEncoding)?
                .to_owned(),
        )
        .ok_or(NewsletterWebhookVerificationError::InvalidHeaderEncoding)?;

        let Some(kind) = event_kind(provider_event_name.as_str()) else {
            return Ok(NewsletterWebhookVerification::Ignored(
                IgnoredNewsletterWebhookEvent {
                    delivery_id,
                    provider_event_name,
                    event_time_unix_seconds,
                },
            ));
        };

        let identity = payload
            .get("contactIdentity")
            .and_then(Value::as_object)
            .ok_or(NewsletterWebhookVerificationError::InvalidEvent)?;
        let provider_contact_id =
            NewsletterWebhookProviderContactId::new(required_string(identity, "id")?.to_owned())
                .ok_or(NewsletterWebhookVerificationError::InvalidEvent)?;
        let email =
            NewsletterWebhookEmailAddress::new(required_string(identity, "email")?.to_owned())
                .ok_or(NewsletterWebhookVerificationError::InvalidEvent)?;
        let mailing_list_id = if matches!(
            kind,
            NewsletterWebhookEventKind::MailingListSubscribed
                | NewsletterWebhookEventKind::MailingListUnsubscribed
        ) {
            let mailing_list = payload
                .get("mailingList")
                .and_then(Value::as_object)
                .ok_or(NewsletterWebhookVerificationError::InvalidEvent)?;
            Some(
                NewsletterWebhookMailingListId::new(
                    required_string(mailing_list, "id")?.to_owned(),
                )
                .ok_or(NewsletterWebhookVerificationError::InvalidEvent)?,
            )
        } else {
            None
        };

        Ok(NewsletterWebhookVerification::Verified(
            VerifiedNewsletterWebhookEvent {
                delivery_id,
                provider_event_name,
                kind,
                event_time_unix_seconds,
                provider_contact_id,
                email,
                mailing_list_id,
            },
        ))
    }
}

fn select_headers(
    headers: &[NewsletterWebhookHeader],
) -> Result<SelectedHeaders<'_>, NewsletterWebhookVerificationError> {
    if headers.len() > MAX_HEADER_COUNT {
        return Err(NewsletterWebhookVerificationError::HeadersTooLarge);
    }
    let mut total_bytes = 0usize;
    let mut selected = SelectedHeaders::default();
    for header in headers {
        if header.name.is_empty()
            || header.name.len() > MAX_HEADER_NAME_BYTES
            || header.value.len() > MAX_HEADER_VALUE_BYTES
        {
            return Err(NewsletterWebhookVerificationError::HeadersTooLarge);
        }
        if !header.name.iter().copied().all(is_header_name_byte) {
            return Err(NewsletterWebhookVerificationError::InvalidHeaderEncoding);
        }
        total_bytes = total_bytes
            .checked_add(header.name.len())
            .and_then(|size| size.checked_add(header.value.len()))
            .ok_or(NewsletterWebhookVerificationError::HeadersTooLarge)?;
        if total_bytes > MAX_HEADER_BYTES {
            return Err(NewsletterWebhookVerificationError::HeadersTooLarge);
        }

        let slot = if header.name.eq_ignore_ascii_case(b"webhook-id") {
            Some(&mut selected.id)
        } else if header.name.eq_ignore_ascii_case(b"webhook-timestamp") {
            Some(&mut selected.timestamp)
        } else if header.name.eq_ignore_ascii_case(b"webhook-signature") {
            Some(&mut selected.signature)
        } else {
            None
        };
        if let Some(slot) = slot {
            if slot.replace(header.value.as_slice()).is_some() {
                return Err(NewsletterWebhookVerificationError::AmbiguousHeader);
            }
        }
    }
    Ok(selected)
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn validate_delivery_id_header(id: &[u8]) -> Result<(), NewsletterWebhookVerificationError> {
    if id.is_empty() || id.len() > 256 || !id.iter().all(|byte| byte.is_ascii_graphic()) {
        return Err(NewsletterWebhookVerificationError::InvalidHeaderEncoding);
    }
    Ok(())
}

fn validate_timestamp_header(timestamp: &[u8]) -> Result<&str, NewsletterWebhookVerificationError> {
    if timestamp.is_empty() || timestamp.len() > 20 || !timestamp.iter().all(u8::is_ascii_digit) {
        return Err(NewsletterWebhookVerificationError::InvalidTimestamp);
    }
    std::str::from_utf8(timestamp)
        .map_err(|_| NewsletterWebhookVerificationError::InvalidHeaderEncoding)
}

fn decode_signing_secret(
    signing_secret: &str,
) -> Result<Vec<u8>, NewsletterWebhookVerificationError> {
    if signing_secret.trim().is_empty() {
        return Err(NewsletterWebhookVerificationError::MissingSigningSecret);
    }
    if signing_secret.len() > MAX_SIGNING_SECRET_BYTES {
        return Err(NewsletterWebhookVerificationError::InvalidSigningSecret);
    }
    let encoded = signing_secret
        .strip_prefix("whsec_")
        .unwrap_or(signing_secret);
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| NewsletterWebhookVerificationError::InvalidSigningSecret)?;
    if decoded.is_empty() {
        return Err(NewsletterWebhookVerificationError::InvalidSigningSecret);
    }
    Ok(decoded)
}

fn parse_signature_candidates(
    signature_header: &[u8],
) -> Result<Vec<Vec<u8>>, NewsletterWebhookVerificationError> {
    if signature_header.is_empty()
        || !signature_header
            .iter()
            .all(|byte| byte.is_ascii() && (!byte.is_ascii_control() || *byte == b'\t'))
    {
        return Err(NewsletterWebhookVerificationError::InvalidSignatureHeader);
    }

    let mut count = 0usize;
    let mut candidates = Vec::new();
    for candidate in signature_header
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|candidate| !candidate.is_empty())
    {
        count += 1;
        if count > MAX_SIGNATURE_CANDIDATES {
            return Err(NewsletterWebhookVerificationError::InvalidSignatureHeader);
        }
        let mut parts = candidate.split(|byte| *byte == b',');
        let version = parts.next().unwrap_or_default();
        let encoded = parts.next().unwrap_or_default();
        if parts.next().is_some()
            || version.is_empty()
            || encoded.is_empty()
            || !version.iter().all(u8::is_ascii_alphanumeric)
        {
            return Err(NewsletterWebhookVerificationError::InvalidSignatureHeader);
        }
        if version == b"v1" {
            let decoded = STANDARD
                .decode(encoded)
                .map_err(|_| NewsletterWebhookVerificationError::InvalidSignatureHeader)?;
            if decoded.len() != SIGNATURE_BYTES {
                return Err(NewsletterWebhookVerificationError::InvalidSignatureHeader);
            }
            candidates.push(decoded);
        }
    }
    if count == 0 {
        return Err(NewsletterWebhookVerificationError::InvalidSignatureHeader);
    }
    Ok(candidates)
}

fn signed_content(id: &[u8], timestamp: &[u8], raw_body: &[u8]) -> Vec<u8> {
    let mut signed = Vec::with_capacity(id.len() + timestamp.len() + raw_body.len() + 2);
    signed.extend_from_slice(id);
    signed.push(b'.');
    signed.extend_from_slice(timestamp);
    signed.push(b'.');
    signed.extend_from_slice(raw_body);
    signed
}

fn hmac_sha256(
    key: &[u8],
    signed_content: &[u8],
) -> Result<Vec<u8>, NewsletterWebhookVerificationError> {
    let key =
        PKey::hmac(key).map_err(|_| NewsletterWebhookVerificationError::CryptographicFailure)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)
        .map_err(|_| NewsletterWebhookVerificationError::CryptographicFailure)?;
    signer
        .update(signed_content)
        .map_err(|_| NewsletterWebhookVerificationError::CryptographicFailure)?;
    signer
        .sign_to_vec()
        .map_err(|_| NewsletterWebhookVerificationError::CryptographicFailure)
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, NewsletterWebhookVerificationError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or(NewsletterWebhookVerificationError::InvalidEvent)
}

fn event_kind(event_name: &str) -> Option<NewsletterWebhookEventKind> {
    match event_name {
        "contact.unsubscribed" => Some(NewsletterWebhookEventKind::ContactUnsubscribed),
        "contact.deleted" => Some(NewsletterWebhookEventKind::ContactDeleted),
        "email.unsubscribed" => Some(NewsletterWebhookEventKind::EmailUnsubscribed),
        "contact.mailingList.subscribed" => Some(NewsletterWebhookEventKind::MailingListSubscribed),
        "contact.mailingList.unsubscribed" => {
            Some(NewsletterWebhookEventKind::MailingListUnsubscribed)
        }
        "email.resubscribed" => Some(NewsletterWebhookEventKind::EmailResubscribed),
        "email.hardBounced" => Some(NewsletterWebhookEventKind::EmailHardBounced),
        "email.spamReported" => Some(NewsletterWebhookEventKind::EmailSpamReported),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DELIVERY_ID: &str = "msg_fixture_001";
    const DELIVERY_TIMESTAMP: &str = "1700000300";
    const ARRIVAL_AT: i64 = 1_700_000_300;
    const SECRET: &str = "whsec_bG9vcHMta2V5LWN1cnJlbnQ=";
    // Golden HMAC values were calculated independently with Python's hmac library.
    const GOLDEN_SIGNATURE: &str = "VOU3nYriMgQ11dwRxAiUlKGF5oZKEXR4XY2DIfyhDu8=";
    const PREVIOUS_KEY_SIGNATURE: &str = "4MSy6fPl0mPAjsgbWBFyLRD8mB7HMIM5sUPRu980bG4=";
    const GOLDEN_BODY: &[u8] = br#"{"eventName":"email.resubscribed","eventTime":1700000000,"webhookSchemaVersion":"1.0.0","contactIdentity":{"id":"contact_123","email":"Ada+Exact@Example.test","userId":"ignored-user-id","auraUserId":"ignored-aura-id"},"sourceType":"campaign","email":{"id":"email_456"}}"#;

    fn verifier() -> LoopsNewsletterWebhookVerifier {
        LoopsNewsletterWebhookVerifier
    }

    fn request(body: &[u8], signature: &str) -> NewsletterWebhookVerificationRequest {
        NewsletterWebhookVerificationRequest {
            headers: vec![
                NewsletterWebhookHeader::new(
                    b"Webhook-Id".to_vec(),
                    DELIVERY_ID.as_bytes().to_vec(),
                ),
                NewsletterWebhookHeader::new(
                    b"Webhook-Timestamp".to_vec(),
                    DELIVERY_TIMESTAMP.as_bytes().to_vec(),
                ),
                NewsletterWebhookHeader::new(
                    b"Webhook-Signature".to_vec(),
                    signature.as_bytes().to_vec(),
                ),
            ],
            raw_body: body.to_vec(),
            signing_secret: SECRET.into(),
            arrived_at_unix_seconds: ARRIVAL_AT,
        }
    }

    fn signature(secret: &str, id: &str, timestamp: &str, body: &[u8]) -> String {
        let key = STANDARD
            .decode(secret.strip_prefix("whsec_").unwrap_or(secret))
            .expect("test signing key");
        let bytes = hmac_sha256(
            &key,
            &signed_content(id.as_bytes(), timestamp.as_bytes(), body),
        )
        .expect("test HMAC");
        STANDARD.encode(bytes)
    }

    fn event_body(event_name: &str, include_mailing_list: bool) -> Vec<u8> {
        let mut payload = json!({
            "eventName": event_name,
            "eventTime": 1699990000,
            "webhookSchemaVersion": "1.0.0",
            "contactIdentity": {
                "id": "contact_exact_1",
                "email": "Exact+Tag@Example.test",
                "userId": "must-not-bind-an-aura-user",
                "auraUserId": "must-not-bind-an-aura-user"
            },
            "providerAddedField": {"anything": true}
        });
        if include_mailing_list {
            payload["mailingList"] = json!({
                "id": "list_exact_1",
                "name": "Main newsletter",
                "description": null,
                "isPublic": true
            });
        }
        serde_json::to_vec(&payload).expect("event JSON")
    }

    fn request_for(body: &[u8], arrival: i64) -> NewsletterWebhookVerificationRequest {
        let sig = signature(SECRET, DELIVERY_ID, DELIVERY_TIMESTAMP, body);
        let mut request = request(body, &format!("v1,{sig}"));
        request.arrived_at_unix_seconds = arrival;
        request
    }

    #[test]
    fn verifies_provider_compatible_golden_hmac_over_the_raw_body() {
        let result = verifier()
            .verify(request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}")))
            .expect("golden request verifies");
        let NewsletterWebhookVerification::Verified(event) = result else {
            panic!("supported event was ignored");
        };
        assert_eq!("msg_fixture_001", event.delivery_id.as_str());
        assert_eq!("email.resubscribed", event.provider_event_name.as_str());
        assert_eq!(NewsletterWebhookEventKind::EmailResubscribed, event.kind);
        assert_eq!(1_700_000_000, event.event_time_unix_seconds);
        assert_eq!("contact_123", event.provider_contact_id.as_str());
        assert_eq!("Ada+Exact@Example.test", event.email.as_str());
        assert_eq!(None, event.mailing_list_id);
        assert_ne!(ARRIVAL_AT, event.event_time_unix_seconds);
    }

    #[test]
    fn rejects_changed_raw_bytes_wrong_key_bad_base64_missing_signature_and_tampering() {
        let mut changed_body = GOLDEN_BODY.to_vec();
        changed_body.push(b' ');
        assert!(matches!(
            verifier().verify(request(&changed_body, &format!("v1,{GOLDEN_SIGNATURE}"))),
            Err(NewsletterWebhookVerificationError::InvalidSignature)
        ));

        let mut wrong_key = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        wrong_key.signing_secret = "whsec_d3Jvbmcta2V5".into();
        assert!(matches!(
            verifier().verify(wrong_key),
            Err(NewsletterWebhookVerificationError::InvalidSignature)
        ));

        assert!(matches!(
            verifier().verify(request(GOLDEN_BODY, "v1,%%%")),
            Err(NewsletterWebhookVerificationError::InvalidSignatureHeader)
        ));

        let mut missing = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        missing.headers.pop();
        assert!(matches!(
            verifier().verify(missing),
            Err(NewsletterWebhookVerificationError::MissingRequiredHeader)
        ));

        for (name, value) in [
            (b"Webhook-Id".as_slice(), b"msg_tampered".as_slice()),
            (b"Webhook-Timestamp".as_slice(), b"1700000301".as_slice()),
        ] {
            let mut tampered = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
            let header = tampered
                .headers
                .iter_mut()
                .find(|header| header.name.eq_ignore_ascii_case(name))
                .expect("header exists");
            header.value = value.to_vec();
            if name.eq_ignore_ascii_case(b"webhook-timestamp") {
                tampered.arrived_at_unix_seconds = 1_700_000_301;
            }
            assert!(matches!(
                verifier().verify(tampered),
                Err(NewsletterWebhookVerificationError::InvalidSignature)
            ));
        }
    }

    #[test]
    fn accepts_any_valid_v1_rotation_candidate_and_rejects_unsupported_versions() {
        let rotating = format!("v1,{PREVIOUS_KEY_SIGNATURE} v1,{GOLDEN_SIGNATURE}");
        assert!(matches!(
            verifier().verify(request(GOLDEN_BODY, &rotating)),
            Ok(NewsletterWebhookVerification::Verified(_))
        ));
        assert!(matches!(
            verifier().verify(request(GOLDEN_BODY, &format!("v2,{GOLDEN_SIGNATURE}"))),
            Err(NewsletterWebhookVerificationError::InvalidSignature)
        ));
        assert!(matches!(
            verifier().verify(request(
                GOLDEN_BODY,
                &format!("v1,{GOLDEN_SIGNATURE},v1,{GOLDEN_SIGNATURE}")
            )),
            Err(NewsletterWebhookVerificationError::InvalidSignatureHeader)
        ));
    }

    #[test]
    fn enforces_stale_and_future_timestamp_tolerance_boundaries() {
        assert!(matches!(
            verifier().verify(request_for(GOLDEN_BODY, ARRIVAL_AT + 300)),
            Ok(NewsletterWebhookVerification::Verified(_))
        ));
        assert!(matches!(
            verifier().verify(request_for(GOLDEN_BODY, ARRIVAL_AT + 301)),
            Err(NewsletterWebhookVerificationError::InvalidTimestamp)
        ));
        assert!(matches!(
            verifier().verify(request_for(GOLDEN_BODY, ARRIVAL_AT - 300)),
            Ok(NewsletterWebhookVerification::Verified(_))
        ));
        assert!(matches!(
            verifier().verify(request_for(GOLDEN_BODY, ARRIVAL_AT - 301)),
            Err(NewsletterWebhookVerificationError::InvalidTimestamp)
        ));
    }

    #[test]
    fn compares_header_names_without_case_and_rejects_ambiguous_duplicates() {
        let mut mixed_case = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        mixed_case.headers[0].name = b"wEbHoOk-iD".to_vec();
        assert!(matches!(
            verifier().verify(mixed_case),
            Ok(NewsletterWebhookVerification::Verified(_))
        ));

        for duplicate_name in [b"WEBHOOK-ID".as_slice(), b"webhook-timestamp".as_slice()] {
            let mut duplicate = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
            duplicate.headers.push(NewsletterWebhookHeader::new(
                duplicate_name.to_vec(),
                b"duplicate".to_vec(),
            ));
            assert!(matches!(
                verifier().verify(duplicate),
                Err(NewsletterWebhookVerificationError::AmbiguousHeader)
            ));
        }

        let mut duplicate_signature = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        duplicate_signature
            .headers
            .push(NewsletterWebhookHeader::new(
                b"WEBHOOK-SIGNATURE".to_vec(),
                format!("v1,{GOLDEN_SIGNATURE}").into_bytes(),
            ));
        assert!(matches!(
            verifier().verify(duplicate_signature),
            Err(NewsletterWebhookVerificationError::AmbiguousHeader)
        ));

        let mut invalid_name = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        invalid_name.headers[0].name = vec![0xff];
        assert!(matches!(
            verifier().verify(invalid_name),
            Err(NewsletterWebhookVerificationError::InvalidHeaderEncoding)
        ));

        let mut invalid_id = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        invalid_id.headers[0].value = vec![0xff];
        assert!(matches!(
            verifier().verify(invalid_id),
            Err(NewsletterWebhookVerificationError::InvalidHeaderEncoding)
        ));
    }

    #[test]
    fn bounds_body_header_and_signature_candidate_sizes() {
        let mut oversized_body = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        oversized_body.raw_body.resize(MAX_BODY_BYTES + 1, b' ');
        assert!(matches!(
            verifier().verify(oversized_body),
            Err(NewsletterWebhookVerificationError::BodyTooLarge)
        ));

        let mut oversized_headers = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        oversized_headers.headers[2].value = vec![b'a'; MAX_HEADER_VALUE_BYTES + 1];
        assert!(matches!(
            verifier().verify(oversized_headers),
            Err(NewsletterWebhookVerificationError::HeadersTooLarge)
        ));

        let mut excessive_total_headers = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        excessive_total_headers.headers.extend([
            NewsletterWebhookHeader::new(b"x-one".to_vec(), vec![b'x'; MAX_HEADER_VALUE_BYTES]),
            NewsletterWebhookHeader::new(b"x-two".to_vec(), vec![b'x'; MAX_HEADER_VALUE_BYTES]),
        ]);
        assert!(matches!(
            verifier().verify(excessive_total_headers),
            Err(NewsletterWebhookVerificationError::HeadersTooLarge)
        ));

        let candidates = std::iter::repeat_n("v2,opaque", MAX_SIGNATURE_CANDIDATES + 1)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(matches!(
            verifier().verify(request(GOLDEN_BODY, &candidates)),
            Err(NewsletterWebhookVerificationError::InvalidSignatureHeader)
        ));
    }

    #[test]
    fn maps_each_supported_event_and_exact_contact_and_list_values() {
        let cases = [
            (
                "contact.unsubscribed",
                NewsletterWebhookEventKind::ContactUnsubscribed,
                false,
            ),
            (
                "contact.deleted",
                NewsletterWebhookEventKind::ContactDeleted,
                false,
            ),
            (
                "email.unsubscribed",
                NewsletterWebhookEventKind::EmailUnsubscribed,
                false,
            ),
            (
                "contact.mailingList.unsubscribed",
                NewsletterWebhookEventKind::MailingListUnsubscribed,
                true,
            ),
            (
                "contact.mailingList.subscribed",
                NewsletterWebhookEventKind::MailingListSubscribed,
                true,
            ),
            (
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
                false,
            ),
            (
                "email.hardBounced",
                NewsletterWebhookEventKind::EmailHardBounced,
                false,
            ),
            (
                "email.spamReported",
                NewsletterWebhookEventKind::EmailSpamReported,
                false,
            ),
        ];
        for (name, expected_kind, has_list) in cases {
            let body = event_body(name, has_list);
            let result = verifier()
                .verify(request_for(&body, ARRIVAL_AT))
                .expect("supported event verifies");
            let NewsletterWebhookVerification::Verified(event) = result else {
                panic!("{name} was ignored");
            };
            assert_eq!(name, event.provider_event_name.as_str());
            assert_eq!(expected_kind, event.kind);
            assert_eq!(1_699_990_000, event.event_time_unix_seconds);
            assert_eq!("contact_exact_1", event.provider_contact_id.as_str());
            assert_eq!("Exact+Tag@Example.test", event.email.as_str());
            assert_eq!(
                has_list.then_some("list_exact_1"),
                event.mailing_list_id.as_ref().map(|list| list.as_str())
            );
            assert_ne!(ARRIVAL_AT, event.event_time_unix_seconds);
        }
    }

    #[test]
    fn rejects_malformed_selected_events_and_unsupported_schemas() {
        for body in [
            br#"{"eventName":"contact.unsubscribed","eventTime":1,"webhookSchemaVersion":"1.0.0"}"#.to_vec(),
            br#"{"eventName":"contact.mailingList.subscribed","eventTime":1,"webhookSchemaVersion":"1.0.0","contactIdentity":{"id":"c","email":"a@example.test"}}"#.to_vec(),
            br#"{"eventName":"contact.unsubscribed","eventTime":"old","webhookSchemaVersion":"1.0.0","contactIdentity":{"id":"c","email":"a@example.test"}}"#.to_vec(),
        ] {
            assert!(matches!(
                verifier().verify(request_for(&body, ARRIVAL_AT)),
                Err(NewsletterWebhookVerificationError::InvalidEvent)
            ));
        }

        let unsupported = br#"{"eventName":"contact.unsubscribed","eventTime":1,"webhookSchemaVersion":"2.0.0","contactIdentity":{"id":"c","email":"a@example.test"}}"#;
        assert!(matches!(
            verifier().verify(request_for(unsupported, ARRIVAL_AT)),
            Err(NewsletterWebhookVerificationError::UnsupportedSchema)
        ));
    }

    #[test]
    fn returns_ignored_for_validly_signed_test_and_unrelated_events() {
        for event_name in ["testing.testEvent", "contact.created", "future.safe.event"] {
            let body = serde_json::to_vec(&json!({
                "eventName": event_name,
                "eventTime": 1699990000,
                "webhookSchemaVersion": "1.0.0",
                "message": "test"
            }))
            .expect("ignored event JSON");
            let result = verifier()
                .verify(request_for(&body, ARRIVAL_AT))
                .expect("valid unrelated event is ignored");
            let NewsletterWebhookVerification::Ignored(event) = result else {
                panic!("unrelated event was mapped to a consent fact");
            };
            assert_eq!(event_name, event.provider_event_name.as_str());
            assert_eq!(1_699_990_000, event.event_time_unix_seconds);
        }
    }

    #[test]
    fn rejects_invalid_secret_and_never_debug_formats_sensitive_values() {
        let mut invalid_secret = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        invalid_secret.signing_secret = "whsec_not-valid-base64".into();
        assert!(matches!(
            verifier().verify(invalid_secret),
            Err(NewsletterWebhookVerificationError::InvalidSigningSecret)
        ));

        let request = request(GOLDEN_BODY, &format!("v1,{GOLDEN_SIGNATURE}"));
        let request_debug = format!("{request:?}");
        for sensitive in [
            SECRET,
            GOLDEN_SIGNATURE,
            "Ada+Exact@Example.test",
            "ignored-user-id",
        ] {
            assert!(!request_debug.contains(sensitive));
        }
        assert!(request_debug.contains("[REDACTED]"));

        let event = verifier().verify(request).expect("golden request verifies");
        let event_debug = format!("{event:?}");
        for sensitive in [
            SECRET,
            GOLDEN_SIGNATURE,
            "Ada+Exact@Example.test",
            "contact_123",
        ] {
            assert!(!event_debug.contains(sensitive));
        }

        let error = NewsletterWebhookVerificationError::InvalidSignature;
        let error_debug = format!("{error:?}");
        for sensitive in [SECRET, GOLDEN_SIGNATURE, "Ada+Exact@Example.test"] {
            assert!(!error_debug.contains(sensitive));
        }
    }
}
