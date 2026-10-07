use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use reqwest::{StatusCode, header::AUTHORIZATION};
use serde::Deserialize;
use serde_email::Email;
use serde_json::{Map, Value};
use user_service::ports::{
    ConsentIntent, ConsentIntentSource, ConsentSubject, MarketingEmailConsentError as Error,
    MarketingEmailConsentOutcome as Outcome, MarketingEmailConsentWriter,
    MarketingEmailSubscriptionState as State,
};

use crate::config::LoopsNewsletterConfig;

const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const SOURCE: &str = "aura-historia-newsletter-api";

#[derive(Deserialize)]
struct UpdateResponse {
    success: bool,
    id: String,
}

#[derive(Deserialize)]
struct RejectionResponse {
    success: bool,
    message: String,
}

#[derive(Deserialize)]
struct SuppressionResponse {
    contact: SuppressionContact,
    #[serde(rename = "isSuppressed")]
    is_suppressed: bool,
}

#[derive(Deserialize)]
struct SuppressionContact {
    id: String,
    email: String,
}

#[derive(Deserialize)]
struct Contact {
    id: String,
    email: String,
    subscribed: bool,
    #[serde(rename = "mailingLists")]
    mailing_lists: HashMap<String, bool>,
    #[serde(rename = "optInStatus")]
    opt_in_status: Option<String>,
}

struct ObservedContact {
    id: String,
    subscribed: bool,
    eligible_opt_in: bool,
    on_target_list: bool,
}

enum SuppressionObservation {
    ContactMissing,
    Present {
        contact_id: String,
        suppressed: bool,
    },
}

pub struct LoopsMarketingEmailConsentWriter {
    config: LoopsNewsletterConfig,
    client: reqwest::Client,
    request_timeout: Duration,
}

impl LoopsMarketingEmailConsentWriter {
    /// One bounded reusable client; the transport must not replay state-changing requests.
    pub fn new(config: LoopsNewsletterConfig) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Protocol { status: None })?;
        Ok(Self {
            config,
            client,
            request_timeout: Duration::from_secs(5),
        })
    }

    // An arbitrary reqwest client cannot be inspected for redirect policy. Keep injection
    // test-only so production always uses the bounded, no-redirect constructor above.
    #[cfg(test)]
    fn with_client(
        config: LoopsNewsletterConfig,
        client: reqwest::Client,
        request_timeout: Duration,
    ) -> Self {
        Self {
            config,
            client,
            request_timeout,
        }
    }

    fn profile_payload(&self, intent: &ConsentIntent) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("email".into(), Value::String(intent.email.to_string()));
        fields.insert("source".into(), Value::String(SOURCE.into()));
        if let Some(profile) = &intent.profile_snapshot {
            if let Some(value) = &profile.first_name {
                fields.insert("firstName".into(), Value::String(value.to_string()));
            }
            if let Some(value) = &profile.last_name {
                fields.insert("lastName".into(), Value::String(value.to_string()));
            }
            if let Some(value) = profile.language {
                fields.insert("language".into(), Value::String(value.as_str().into()));
            }
            if let Some(value) = profile.currency {
                fields.insert("currency".into(), Value::String(value.as_str().into()));
            }
        }
        if let ConsentSubject::User(id) = intent.subject {
            fields.insert("auraUserId".into(), Value::String(id.to_string()));
        }
        fields
    }

    fn consent_payload(&self, intent: &ConsentIntent, grant: bool) -> Value {
        let mut fields = if grant {
            self.profile_payload(intent)
        } else {
            let mut fields = Map::new();
            fields.insert("email".into(), Value::String(intent.email.to_string()));
            fields
        };
        fields.insert("subscribed".into(), Value::Bool(grant));
        fields.insert(
            "mailingLists".into(),
            Value::Object(Map::from_iter([(
                self.config.newsletter_list_id.clone(),
                Value::Bool(grant),
            )])),
        );
        Value::Object(fields)
    }

    async fn send_update(&self, payload: Value) -> Result<String, Error> {
        let response = self
            .client
            .put(self.config.update_url.clone())
            .header(AUTHORIZATION, self.config.authorization.clone())
            .timeout(self.request_timeout)
            .json(&payload)
            .send()
            .await
            .map_err(|error| {
                if error.is_builder() {
                    Error::Protocol { status: None }
                } else if error.is_connect() && !error.is_timeout() {
                    Error::NotSent
                } else {
                    Error::AcceptanceUnknown
                }
            })?;
        let status = response.status();
        let code = Some(status.as_u16());
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::Throttled { status: code });
        }
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            return Err(Error::AcceptanceUnknown);
        }
        if status == StatusCode::BAD_REQUEST {
            return Err(if invalid_email_response(response).await? {
                Error::InvalidEmail
            } else {
                Error::Rejected { status: code }
            });
        }
        if status != StatusCode::OK {
            return Err(Error::Protocol { status: code });
        }
        let body = read_bounded(response).await.map_err(|error| match error {
            Error::ReadUnavailable => Error::AcceptanceUnknown,
            _ => error,
        })?;
        let accepted: UpdateResponse =
            serde_json::from_slice(&body).map_err(|_| Error::Protocol { status: code })?;
        if !accepted.success || accepted.id.trim().is_empty() {
            return Err(Error::Protocol { status: code });
        }
        Ok(accepted.id)
    }

    async fn suppression(&self, email: &Email) -> Result<SuppressionObservation, Error> {
        let mut url = self.config.suppression_url.clone();
        url.query_pairs_mut().append_pair("email", email.as_ref());
        let response = self
            .client
            .get(url)
            .header(AUTHORIZATION, self.config.authorization.clone())
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| Error::ReadUnavailable)?;
        let status = response.status();
        let code = Some(status.as_u16());
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::Throttled { status: code });
        }
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            return Err(Error::ReadUnavailable);
        }
        if status == StatusCode::NOT_FOUND {
            return Ok(SuppressionObservation::ContactMissing);
        }
        if status == StatusCode::BAD_REQUEST {
            return Err(read_bad_request(response).await);
        }
        if status != StatusCode::OK {
            return Err(Error::Protocol { status: code });
        }
        let body = read_bounded(response).await?;
        let result: SuppressionResponse =
            serde_json::from_slice(&body).map_err(|_| Error::Protocol { status: code })?;
        if result.contact.id.trim().is_empty() || result.contact.email != email.as_ref() {
            return Err(Error::Protocol { status: code });
        }
        Ok(SuppressionObservation::Present {
            contact_id: result.contact.id,
            suppressed: result.is_suppressed,
        })
    }

    async fn find_contact(&self, email: &Email) -> Result<Option<ObservedContact>, Error> {
        let mut url = self.config.find_url.clone();
        url.query_pairs_mut().append_pair("email", email.as_ref());
        let response = self
            .client
            .get(url)
            .header(AUTHORIZATION, self.config.authorization.clone())
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|error| {
                if error.is_builder() {
                    Error::Protocol { status: None }
                } else {
                    Error::ReadUnavailable
                }
            })?;
        let status = response.status();
        let code = Some(status.as_u16());
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::Throttled { status: code });
        }
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            return Err(Error::ReadUnavailable);
        }
        if status == StatusCode::BAD_REQUEST {
            return Err(read_bad_request(response).await);
        }
        if status != StatusCode::OK {
            return Err(Error::Protocol { status: code });
        }
        let body = read_bounded(response).await?;
        let contacts: Vec<Contact> =
            serde_json::from_slice(&body).map_err(|_| Error::Protocol { status: code })?;
        match contacts.as_slice() {
            [] => Ok(None),
            [contact] if !contact.id.trim().is_empty() && contact.email == email.as_ref() => {
                // A pending/rejected provider DOI is not marketing eligibility.
                Ok(Some(ObservedContact {
                    id: contact.id.clone(),
                    subscribed: contact.subscribed,
                    eligible_opt_in: contact
                        .opt_in_status
                        .as_deref()
                        .is_none_or(|v| v == "accepted"),
                    on_target_list: contact
                        .mailing_lists
                        .get(&self.config.newsletter_list_id)
                        .copied()
                        .unwrap_or(false),
                }))
            }
            _ => Err(Error::Protocol { status: code }),
        }
    }
}

async fn invalid_email_response(response: reqwest::Response) -> Result<bool, Error> {
    let body = read_bounded(response).await?;
    let rejected: RejectionResponse =
        serde_json::from_slice(&body).map_err(|_| Error::Protocol { status: Some(400) })?;
    Ok(!rejected.success && rejected.message == "Invalid email address.")
}

async fn read_bad_request(response: reqwest::Response) -> Error {
    match invalid_email_response(response).await {
        Ok(true) => Error::InvalidEmail,
        _ => Error::Protocol { status: Some(400) },
    }
}

async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, Error> {
    let status = Some(response.status().as_u16());
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Error::Protocol { status });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::ReadUnavailable)? {
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return Err(Error::Protocol { status });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn readback_error(error: Error) -> Error {
    match error {
        Error::Protocol { .. } => error,
        _ => Error::AcceptanceUnknown,
    }
}

#[async_trait]
impl MarketingEmailConsentWriter for LoopsMarketingEmailConsentWriter {
    async fn current_state(&self, email: &Email) -> Result<State, Error> {
        let Some(contact) = self.find_contact(email).await? else {
            return Ok(State::Missing);
        };
        let suppressed = match self.suppression(email).await? {
            SuppressionObservation::Present {
                contact_id,
                suppressed,
            } if contact_id == contact.id => suppressed,
            _ => return Err(Error::Protocol { status: None }),
        };
        Ok(State::Present {
            contact_id: contact.id,
            suppressed,
            globally_subscribed: contact.subscribed && contact.eligible_opt_in,
            on_target_list: contact.on_target_list,
        })
    }

    async fn grant(&self, intent: &ConsentIntent) -> Result<Outcome, Error> {
        if !intent.desired || intent.source == ConsentIntentSource::ProviderRaceRepair {
            return Err(Error::IneligibleIntent);
        }
        let before = self.find_contact(&intent.email).await?;
        let suppression = self.suppression(&intent.email).await?;
        match (&before, suppression) {
            (None, SuppressionObservation::ContactMissing) => {}
            (
                Some(contact),
                SuppressionObservation::Present {
                    contact_id,
                    suppressed,
                },
            ) if contact.id == contact_id => {
                if suppressed {
                    return Ok(Outcome::BlockedByProviderPreferences);
                }
            }
            _ => return Err(Error::Protocol { status: None }),
        }
        if let Some(contact) = before
            && contact.subscribed
            && contact.eligible_opt_in
            && contact.on_target_list
        {
            return Ok(Outcome::AlreadyApplied {
                contact_id: Some(contact.id),
            });
        }
        let id = self.send_update(self.consent_payload(intent, true)).await?;
        // Both independent reads are required after acceptance, even if find returns Missing.
        let after = self.find_contact(&intent.email).await;
        let suppression = self.suppression(&intent.email).await;
        match (
            after.map_err(readback_error)?,
            suppression.map_err(readback_error)?,
        ) {
            (
                Some(contact),
                SuppressionObservation::Present {
                    contact_id,
                    suppressed,
                },
            ) if contact.id == id && contact_id == id => {
                if !suppressed
                    && contact.subscribed
                    && contact.eligible_opt_in
                    && contact.on_target_list
                {
                    Ok(Outcome::Applied { contact_id: id })
                } else {
                    Ok(Outcome::BlockedByProviderPreferences)
                }
            }
            _ => Err(Error::AcceptanceUnknown),
        }
    }

    async fn revoke(&self, intent: &ConsentIntent) -> Result<Outcome, Error> {
        if intent.desired {
            return Err(Error::IneligibleIntent);
        }
        match self.find_contact(&intent.email).await? {
            None => return Ok(Outcome::AlreadyApplied { contact_id: None }),
            Some(contact) if !contact.subscribed && !contact.on_target_list => {
                return Ok(Outcome::AlreadyApplied {
                    contact_id: Some(contact.id),
                });
            }
            _ => {}
        }
        let id = self
            .send_update(self.consent_payload(intent, false))
            .await?;
        match self
            .find_contact(&intent.email)
            .await
            .map_err(readback_error)?
        {
            None => Ok(Outcome::Applied { contact_id: id }),
            Some(contact) if contact.id == id && !contact.subscribed && !contact.on_target_list => {
                Ok(Outcome::Applied { contact_id: id })
            }
            _ => Err(Error::AcceptanceUnknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use user_core::{
        first_name::FirstName, marketing_consent_sync_intent_id::MarketingConsentSyncIntentId,
        user_id::UserId,
    };
    use user_service::ports::NewsletterProfile;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    const EMAIL: &str = "ada+one@example.com";
    const LIST: &str = "dynamic-purpose-id";
    const ID: &str = "contact-123";

    fn intent(desired: bool) -> ConsentIntent {
        ConsentIntent {
            intent_id: MarketingConsentSyncIntentId::new(),
            source_key: "synthetic-proof".into(),
            subject: ConsentSubject::EmailOnly,
            source: ConsentIntentSource::AuraDoubleOptIn,
            email: EMAIL.try_into().unwrap(),
            profile_snapshot: None,
            desired,
        }
    }

    fn writer(server: &MockServer) -> LoopsMarketingEmailConsentWriter {
        let config = LoopsNewsletterConfig::new(
            "synthetic-key".into(),
            LIST.into(),
            format!("{}/api/", server.uri()),
        )
        .unwrap();
        LoopsMarketingEmailConsentWriter::new(config).unwrap()
    }

    fn contact(global: bool, list: bool) -> Value {
        json!([{"id": ID, "email": EMAIL, "subscribed": global, "mailingLists": {(LIST): list}, "optInStatus": "accepted"}])
    }

    async fn suppression_mock(server: &MockServer, suppressed: bool) {
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/suppression"))
            .and(query_param("email", EMAIL))
            .and(header("authorization", "Bearer synthetic-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "contact": {"id": ID, "email": EMAIL},
                "isSuppressed": suppressed,
                "removalQuota": {"limit": 5, "remaining": 5}
            })))
            .mount(server)
            .await;
    }

    async fn missing_suppression_mock(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/suppression"))
            .and(query_param("email", EMAIL))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "success": false, "message": "Contact not found"
            })))
            .mount(server)
            .await;
    }

    async fn transitioning_suppression(server: &MockServer, after: ResponseTemplate) {
        let reads = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/suppression"))
            .and(query_param("email", EMAIL))
            .respond_with(move |_: &wiremock::Request| {
                if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(404).set_body_json(json!({
                        "success": false, "message": "Contact not found"
                    }))
                } else {
                    after.clone()
                }
            })
            .expect(2)
            .mount(server)
            .await;
    }

    fn unsuppressed() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "contact": {"id": ID, "email": EMAIL}, "isSuppressed": false
        }))
    }

    async fn find_mock(server: &MockServer, body: Value) {
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/find"))
            .and(query_param("email", EMAIL))
            .and(header("authorization", "Bearer synthetic-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    async fn transitioning_find(server: &MockServer, before: Value, after: Value) {
        let reads = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/find"))
            .and(query_param("email", EMAIL))
            .and(header("authorization", "Bearer synthetic-key"))
            .respond_with(move |_: &wiremock::Request| {
                let state = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    &before
                } else {
                    &after
                };
                ResponseTemplate::new(200).set_body_json(state)
            })
            .expect(2)
            .mount(server)
            .await;
    }

    async fn requests_to(server: &MockServer, endpoint: &str) -> usize {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == endpoint)
            .count()
    }

    fn accepted() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "id": ID}))
    }

    #[tokio::test]
    async fn fresh_grant_sends_only_exact_email_and_confirmed_snapshot_then_reads_target_state() {
        let server = MockServer::start().await;
        find_mock(&server, contact(false, false)).await;
        suppression_mock(&server, false).await;
        let writer = writer(&server);
        let mut intent = intent(true);
        intent.subject = ConsentSubject::User(UserId::new());
        intent.profile_snapshot = Some(Box::new(NewsletterProfile {
            first_name: Some(FirstName::from("Ada")),
            last_name: None,
            language: Some(Language::En),
            currency: Some(Currency::Eur),
        }));
        let payload = writer.consent_payload(&intent, true);
        assert_eq!(payload["email"], EMAIL);
        assert_eq!(payload["firstName"], "Ada");
        assert_eq!(payload["language"], "en");
        assert_eq!(payload["currency"], "EUR");
        let ConsentSubject::User(user_id) = intent.subject else {
            panic!("expected user");
        };
        assert_eq!(payload["auraUserId"], user_id.to_string());
        assert!(payload.get("lastName").is_none());
        assert!(payload.get("userId").is_none());
        assert_eq!(payload["mailingLists"], json!({(LIST): true}));
        assert!(writer.profile_payload(&intent).get("subscribed").is_none());
        assert!(
            writer
                .profile_payload(&intent)
                .get("mailingLists")
                .is_none()
        );
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .and(body_json(Value::Object(
                payload.as_object().unwrap().clone(),
            )))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        // A persistent stale GET is not proof of delivery, even with HTTP 200.
        assert_eq!(
            writer.grant(&intent).await.unwrap(),
            Outcome::BlockedByProviderPreferences
        );
    }

    #[tokio::test]
    async fn grant_applies_only_after_readback_confirms_both_flags_and_no_suppression() {
        let server = MockServer::start().await;
        transitioning_find(&server, contact(false, false), contact(true, true)).await;
        suppression_mock(&server, false).await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap(),
            Outcome::Applied {
                contact_id: ID.into()
            }
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 2);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            2
        );
    }

    #[tokio::test]
    async fn suppressed_contact_is_blocked_before_grant_even_when_flags_are_true() {
        let server = MockServer::start().await;
        find_mock(&server, contact(true, true)).await;
        suppression_mock(&server, true).await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap(),
            Outcome::BlockedByProviderPreferences
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method.as_str() == "GET")
        );
    }

    #[tokio::test]
    async fn missing_find_with_present_suppression_never_grants() {
        let server = MockServer::start().await;
        find_mock(&server, json!([])).await;
        suppression_mock(&server, false).await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::Protocol { status: None }
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 1);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            1
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);
    }

    #[tokio::test]
    async fn existing_find_with_missing_suppression_never_grants_or_reports_unsuppressed() {
        let server = MockServer::start().await;
        find_mock(&server, contact(true, true)).await;
        missing_suppression_mock(&server).await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::Protocol { status: None }
        );
        assert_eq!(
            writer(&server)
                .current_state(&intent(true).email)
                .await
                .unwrap_err(),
            Error::Protocol { status: None }
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);
    }

    #[tokio::test]
    async fn fresh_contact_grants_after_missing_suppression_then_coherent_readback() {
        let server = MockServer::start().await;
        transitioning_find(&server, json!([]), contact(true, true)).await;
        transitioning_suppression(&server, unsuppressed()).await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap(),
            Outcome::Applied {
                contact_id: ID.into()
            }
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 2);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            2
        );
    }

    #[tokio::test]
    async fn inconclusive_missing_contact_suppression_never_grants() {
        for response in [
            ResponseTemplate::new(503),
            ResponseTemplate::new(200).set_body_json(json!({"isSuppressed": false})),
        ] {
            let server = MockServer::start().await;
            find_mock(&server, json!([])).await;
            Mock::given(method("GET"))
                .and(path("/api/v1/contacts/suppression"))
                .and(query_param("email", EMAIL))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            assert!(matches!(
                writer(&server).grant(&intent(true)).await,
                Err(Error::ReadUnavailable | Error::Protocol { .. })
            ));
            assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);
        }
    }

    #[tokio::test]
    async fn suppression_read_must_match_the_found_contact() {
        let server = MockServer::start().await;
        find_mock(&server, contact(true, true)).await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/suppression"))
            .and(query_param("email", EMAIL))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "contact": {"id": "different-contact", "email": EMAIL}, "isSuppressed": false
            })))
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::Protocol { status: None }
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method.as_str() == "GET")
        );
    }

    #[tokio::test]
    async fn grant_can_apply_and_already_applied_requires_both_flags() {
        let server = MockServer::start().await;
        find_mock(&server, contact(true, true)).await;
        suppression_mock(&server, false).await;
        let writer = writer(&server);
        assert_eq!(
            writer.grant(&intent(true)).await.unwrap(),
            Outcome::AlreadyApplied {
                contact_id: Some(ID.into())
            }
        );
        assert_eq!(
            writer.current_state(&intent(true).email).await.unwrap(),
            State::Present {
                contact_id: ID.into(),
                globally_subscribed: true,
                on_target_list: true,
                suppressed: false
            }
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method.as_str() == "GET")
        );
    }

    #[tokio::test]
    async fn global_true_list_false_remains_blocked_even_after_successful_update() {
        let server = MockServer::start().await;
        find_mock(&server, contact(true, false)).await;
        suppression_mock(&server, false).await;
        Mock::given(method("PUT")).and(body_json(json!({"email": EMAIL, "source": SOURCE, "subscribed": true, "mailingLists": {(LIST): true}})))
        .respond_with(accepted()).expect(1).mount(&server).await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap(),
            Outcome::BlockedByProviderPreferences
        );
    }

    #[tokio::test]
    async fn revoke_missing_never_upserts_and_repair_uses_same_revoke_payload() {
        let server = MockServer::start().await;
        find_mock(&server, json!([])).await;
        let writer = writer(&server);
        let mut repair = intent(false);
        repair.source = ConsentIntentSource::ProviderRaceRepair;
        assert_eq!(
            writer.revoke(&repair).await.unwrap(),
            Outcome::AlreadyApplied { contact_id: None }
        );
        assert_eq!(
            writer.consent_payload(&repair, false),
            json!({"email": EMAIL, "subscribed": false, "mailingLists": {(LIST): false}})
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 1);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            0
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);
    }

    #[tokio::test]
    async fn already_safe_revoke_needs_neither_suppression_nor_update() {
        let server = MockServer::start().await;
        find_mock(&server, contact(false, false)).await;
        assert_eq!(
            writer(&server).revoke(&intent(false)).await.unwrap(),
            Outcome::AlreadyApplied {
                contact_id: Some(ID.into())
            }
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 1);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            0
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);
    }

    #[tokio::test]
    async fn revoke_existing_uses_explicit_false_flags_and_never_touches_other_lists() {
        let server = MockServer::start().await;
        find_mock(&server, contact(true, true)).await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .and(body_json(
                json!({"email": EMAIL, "subscribed": false, "mailingLists": {(LIST): false}}),
            ))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).revoke(&intent(false)).await.unwrap_err(),
            Error::AcceptanceUnknown
        );
    }

    #[tokio::test]
    async fn revoke_applies_only_after_false_flags_readback() {
        let server = MockServer::start().await;
        transitioning_find(&server, contact(true, true), contact(false, false)).await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .and(body_json(
                json!({"email": EMAIL, "subscribed": false, "mailingLists": {(LIST): false}}),
            ))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).revoke(&intent(false)).await.unwrap(),
            Outcome::Applied {
                contact_id: ID.into()
            }
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 2);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            0
        );
    }

    #[tokio::test]
    async fn suppressed_grant_readback_does_not_claim_applied() {
        let server = MockServer::start().await;
        transitioning_find(&server, json!([]), contact(true, true)).await;
        transitioning_suppression(
            &server,
            ResponseTemplate::new(200).set_body_json(json!({
                "contact": {"id": ID, "email": EMAIL}, "isSuppressed": true
            })),
        )
        .await;
        Mock::given(method("PUT"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap(),
            Outcome::BlockedByProviderPreferences
        );
    }

    #[tokio::test]
    async fn missing_post_grant_find_with_suppressed_expected_contact_is_uncertain() {
        let server = MockServer::start().await;
        transitioning_find(&server, json!([]), json!([])).await;
        transitioning_suppression(
            &server,
            ResponseTemplate::new(200).set_body_json(json!({
                "contact": {"id": ID, "email": EMAIL}, "isSuppressed": true
            })),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;

        let result = writer(&server).grant(&intent(true)).await;
        assert_ne!(result, Ok(Outcome::BlockedByProviderPreferences));
        assert_eq!(result, Err(Error::AcceptanceUnknown));
    }

    #[tokio::test]
    async fn different_post_grant_find_id_with_suppressed_expected_contact_is_uncertain() {
        let server = MockServer::start().await;
        transitioning_find(
            &server,
            json!([]),
            json!([{"id": "other-contact", "email": EMAIL, "subscribed": true,
                "mailingLists": {(LIST): true}, "optInStatus": "accepted"}]),
        )
        .await;
        transitioning_suppression(
            &server,
            ResponseTemplate::new(200).set_body_json(json!({
                "contact": {"id": ID, "email": EMAIL}, "isSuppressed": true
            })),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            writer(&server).grant(&intent(true)).await,
            Err(Error::AcceptanceUnknown)
        );
    }

    #[tokio::test]
    async fn missing_grant_readback_is_uncertain() {
        let server = MockServer::start().await;
        transitioning_find(&server, json!([]), json!([])).await;
        transitioning_suppression(&server, unsuppressed()).await;
        Mock::given(method("PUT"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::AcceptanceUnknown
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 2);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            2
        );
    }

    #[tokio::test]
    async fn accepted_grant_with_unavailable_post_suppression_is_not_applied() {
        let server = MockServer::start().await;
        transitioning_find(&server, json!([]), contact(true, true)).await;
        transitioning_suppression(&server, ResponseTemplate::new(503)).await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::AcceptanceUnknown
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 2);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            2
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 1);
    }

    #[tokio::test]
    async fn accepted_grant_with_post_suppression_missing_is_uncertain() {
        let server = MockServer::start().await;
        transitioning_find(&server, contact(false, false), contact(true, true)).await;
        let reads = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/suppression"))
            .and(query_param("email", EMAIL))
            .respond_with(move |_: &wiremock::Request| {
                if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    unsuppressed()
                } else {
                    ResponseTemplate::new(404).set_body_json(json!({
                        "success": false, "message": "Contact not found"
                    }))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(accepted())
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::AcceptanceUnknown
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/find").await, 2);
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            2
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 1);
    }

    #[tokio::test]
    async fn read_invalid_email_is_classified_and_redacted_before_write() {
        let invalid = json!({"success": false, "message": "Invalid email address."});
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/find"))
            .and(query_param("email", EMAIL))
            .respond_with(ResponseTemplate::new(400).set_body_json(&invalid))
            .expect(1)
            .mount(&server)
            .await;
        let error = writer(&server).grant(&intent(true)).await.unwrap_err();
        assert_eq!(error, Error::InvalidEmail);
        assert!(!format!("{error:?} {error}").contains(EMAIL));
        assert!(!format!("{error:?} {error}").contains("synthetic-key"));
        assert_eq!(
            requests_to(&server, "/api/v1/contacts/suppression").await,
            0
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);

        let server = MockServer::start().await;
        find_mock(&server, contact(false, false)).await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/suppression"))
            .and(query_param("email", EMAIL))
            .respond_with(ResponseTemplate::new(400).set_body_json(&invalid))
            .expect(1)
            .mount(&server)
            .await;
        let error = writer(&server).grant(&intent(true)).await.unwrap_err();
        assert_eq!(error, Error::InvalidEmail);
        assert!(!format!("{error:?} {error}").contains(EMAIL));
        assert!(!format!("{error:?} {error}").contains("synthetic-key"));
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);
    }

    #[tokio::test]
    async fn malformed_read_bad_requests_fail_closed() {
        for endpoint in ["find", "suppression"] {
            let server = MockServer::start().await;
            if endpoint == "suppression" {
                find_mock(&server, contact(false, false)).await;
            }
            Mock::given(method("GET"))
                .and(path(format!("/api/v1/contacts/{endpoint}")))
                .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                    "success": false, "message": "other"
                })))
                .mount(&server)
                .await;
            assert_eq!(
                writer(&server).grant(&intent(true)).await.unwrap_err(),
                Error::Protocol { status: Some(400) }
            );
            assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 0);
        }
    }

    #[tokio::test]
    async fn injected_client_classifies_prewrite_connection_failure_as_not_sent() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/api", listener.local_addr().unwrap());
        let config =
            LoopsNewsletterConfig::new("synthetic-key".into(), LIST.into(), base_url).unwrap();
        drop(listener);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let writer = LoopsMarketingEmailConsentWriter::with_client(
            config,
            client,
            Duration::from_millis(50),
        );
        assert_eq!(
            writer
                .send_update(writer.consent_payload(&intent(true), true))
                .await
                .unwrap_err(),
            Error::NotSent
        );
    }

    #[tokio::test]
    async fn injected_client_classifies_write_timeout_as_acceptance_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(accepted().set_delay(Duration::from_millis(250)))
            .mount(&server)
            .await;
        let config = LoopsNewsletterConfig::new(
            "synthetic-key".into(),
            LIST.into(),
            format!("{}/api", server.uri()),
        )
        .unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let writer = LoopsMarketingEmailConsentWriter::with_client(
            config,
            client,
            Duration::from_millis(50),
        );
        assert_eq!(
            writer
                .send_update(writer.consent_payload(&intent(true), true))
                .await
                .unwrap_err(),
            Error::AcceptanceUnknown
        );
        assert_eq!(requests_to(&server, "/api/v1/contacts/update").await, 1);
    }

    #[tokio::test]
    async fn classified_rejections_and_ambiguous_acceptance_are_redacted() {
        for (status, body, expected) in [
            (
                400,
                json!({"success": false, "message": "Invalid email address."}),
                Error::InvalidEmail,
            ),
            (
                400,
                json!({"success": false, "message": "other"}),
                Error::Rejected { status: Some(400) },
            ),
            (429, json!({}), Error::Throttled { status: Some(429) }),
            (503, json!({}), Error::AcceptanceUnknown),
            (302, json!({}), Error::Protocol { status: Some(302) }),
            (405, json!({}), Error::Protocol { status: Some(405) }),
            (404, json!({}), Error::Protocol { status: Some(404) }),
            (
                200,
                json!({"success": true, "id": ""}),
                Error::Protocol { status: Some(200) },
            ),
            (
                200,
                json!({"success": true}),
                Error::Protocol { status: Some(200) },
            ),
        ] {
            let server = MockServer::start().await;
            find_mock(&server, contact(false, false)).await;
            suppression_mock(&server, false).await;
            Mock::given(method("PUT"))
                .and(path("/api/v1/contacts/update"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let error = writer(&server).grant(&intent(true)).await.unwrap_err();
            assert_eq!(error, expected);
            let displayed = format!("{error:?} {error}");
            assert!(!displayed.contains(EMAIL));
            assert!(!displayed.contains("synthetic-key"));
        }
    }

    #[tokio::test]
    async fn oversized_or_redirected_write_never_counts_as_success_or_forwards_credentials() {
        let server = MockServer::start().await;
        find_mock(&server, contact(false, false)).await;
        suppression_mock(&server, false).await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("x".repeat(MAX_RESPONSE_BYTES + 1), "text/plain"),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::Protocol { status: Some(200) }
        );

        let server = MockServer::start().await;
        find_mock(&server, contact(false, false)).await;
        suppression_mock(&server, false).await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/capture", server.uri()).as_str()),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server).grant(&intent(true)).await.unwrap_err(),
            Error::Protocol { status: Some(307) }
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.url.path() != "/capture")
        );
    }

    #[tokio::test]
    async fn oversized_read_and_redirects_fail_closed_without_leaking_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/find"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("x".repeat(MAX_RESPONSE_BYTES + 1), "text/plain"),
            )
            .mount(&server)
            .await;
        let error = writer(&server)
            .current_state(&intent(true).email)
            .await
            .unwrap_err();
        assert_eq!(error, Error::Protocol { status: Some(200) });
        assert!(!format!("{error:?} {error}").contains(EMAIL));

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/contacts/find"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://other.example/"),
            )
            .mount(&server)
            .await;
        assert_eq!(
            writer(&server)
                .current_state(&intent(true).email)
                .await
                .unwrap_err(),
            Error::Protocol { status: Some(302) }
        );
    }
}
