use super::*;

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_publish_public_exact_discovery_without_authentication() {
    let client = reqwest::Client::new();
    let url = format!(
        "{}/.well-known/oauth-authorization-server",
        AURA_API.base_url()
    );
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(reqwest::StatusCode::OK, response.status());
    assert_eq!(
        "no-store",
        response.headers()[reqwest::header::CACHE_CONTROL]
    );
    let (_, metadata) = json_response(response).await;
    assert_eq!("https://api.aura-historia.com", metadata["issuer"]);
    assert_eq!(
        "https://aura-historia.com/en/oauth/authorize",
        metadata["authorization_endpoint"]
    );
    assert_eq!(
        "https://api.aura-historia.com/v1/oauth/token",
        metadata["token_endpoint"]
    );
    assert_eq!(
        serde_json::json!(["authorization_code"]),
        metadata["grant_types_supported"]
    );
    assert_eq!(
        serde_json::json!(["client_secret_post"]),
        metadata["token_endpoint_auth_methods_supported"]
    );
    assert_eq!(
        serde_json::json!(["S256"]),
        metadata["code_challenge_methods_supported"]
    );
    for unsupported in [
        "registration_endpoint",
        "revocation_endpoint",
        "introspection_endpoint",
        "authorization_response_iss_parameter_supported",
        "client_id_metadata_document_supported",
    ] {
        assert!(metadata.get(unsupported).is_none(), "{unsupported}");
    }
    let response = client
        .get(&url)
        .bearer_auth("not-an-aura-credential")
        .send()
        .await
        .unwrap();
    let (status, with_credentials) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(metadata, with_credentials);
    for alias in [
        "/v1/.well-known/oauth-authorization-server",
        "/api/v1/oauth/token",
        "/.well-known/oauth-authorization-server/",
    ] {
        let response = client
            .get(format!("{}{alias}", AURA_API.base_url()))
            .send()
            .await
            .unwrap();
        assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_return_standard_secret_free_token_errors_at_the_discovered_path() {
    let admin = oauth_admin().await;
    let credentials = create_oauth_client(&admin.client, &admin.cognito_token).await;
    let (_, metadata) = json_response(
        admin
            .client
            .get(format!(
                "{}/.well-known/oauth-authorization-server",
                AURA_API.base_url()
            ))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let endpoint = url::Url::parse(metadata["token_endpoint"].as_str().unwrap()).unwrap();
    // Exercise discovered routing against isolated local fixtures, without deployed services.
    let endpoint = format!("{}{}", AURA_API.base_url(), endpoint.path());
    let code = authorize_code(&admin.client, &admin.cognito_token, &credentials).await;
    let valid = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code),
        ("redirect_uri", "https://client.example/callback".to_owned()),
        ("client_id", credentials.client_id.to_string()),
        ("client_secret", credentials.client_secret.clone()),
        ("code_verifier", PKCE_CODE_VERIFIER.to_owned()),
    ];
    let marker = "synthetic-secret-must-not-be-reflected";
    for (field, value, expected_status, error) in [
        (
            "grant_type",
            marker.to_owned(),
            400,
            "unsupported_grant_type",
        ),
        ("code", marker.to_owned(), 400, "invalid_grant"),
        (
            "code",
            OAuthAuthorizationCode::new().to_string(),
            400,
            "invalid_grant",
        ),
        ("code_verifier", marker.to_owned(), 400, "invalid_grant"),
        (
            "client_id",
            OAuthClientId::new().to_string(),
            401,
            "invalid_client",
        ),
        ("client_secret", marker.to_owned(), 401, "invalid_client"),
        (
            "client_secret",
            String::from(user_core::access_token::RawOAuthClientSecret::new()),
            401,
            "invalid_client",
        ),
        (
            "resource",
            "https://foreign.example".to_owned(),
            400,
            "invalid_target",
        ),
        ("code_verifier", "x".repeat(43), 400, "invalid_grant"),
    ] {
        let mut form = valid.clone();
        form.retain(|(key, _)| *key != field);
        form.push((field, value.clone()));
        let response = admin
            .client
            .post(&endpoint)
            .form(&form)
            .send()
            .await
            .unwrap();
        assert_credential_headers(&response);
        assert_eq!(
            "application/json",
            response.headers()[reqwest::header::CONTENT_TYPE]
        );
        let (status, body) = json_response(response).await;
        assert_eq!(expected_status, status.as_u16(), "{field}");
        assert_eq!(serde_json::json!({"error": error}), body);
        assert!(!body.to_string().contains(marker));
        assert!(!body.to_string().contains(&credentials.client_secret));
    }
    for body in [
        "grant_type=authorization_code&client_id=a&client_id=b",
        "resource=allowed&resource=foreign",
    ] {
        let response = admin
            .client
            .post(&endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .unwrap();
        assert_credential_headers(&response);
        assert_eq!(reqwest::StatusCode::BAD_REQUEST, response.status());
    }
    let response = admin
        .client
        .post(&endpoint)
        .json(&serde_json::json!({"grant_type": "authorization_code"}))
        .send()
        .await
        .unwrap();
    assert_credential_headers(&response);
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::BAD_REQUEST, status);
    assert_eq!(serde_json::json!({"error": "invalid_request"}), body);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_reject_invalid_authorization_scope_without_reflecting_it() {
    let admin = oauth_admin().await;
    let credentials = create_oauth_client(&admin.client, &admin.cognito_token).await;
    for scope in [
        "offline_access",
        "users:read",
        "synthetic-secret-must-not-be-reflected",
    ] {
        let response = admin
            .client
            .get(format!("{}/v1/oauth/authorize", AURA_API.base_url()))
            .bearer_auth(&admin.cognito_token)
            .query(&[
                ("response_type", "code"),
                ("client_id", credentials.client_id.to_string().as_str()),
                ("redirect_uri", "https://client.example/callback"),
                ("scope", scope),
                ("code_challenge", PKCE_CODE_CHALLENGE),
                ("code_challenge_method", "S256"),
            ])
            .send()
            .await
            .unwrap();
        assert_credential_headers(&response);
        assert!(response.headers().get(reqwest::header::LOCATION).is_none());
        let (status, body) = json_response(response).await;
        assert_eq!(reqwest::StatusCode::BAD_REQUEST, status);
        assert_eq!(serde_json::json!({"error": "invalid_scope"}), body);
    }
}
