use super::*;

const RELEASE: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn should_report_the_release_identity_in_current_and_legacy_discovery() {
    let app = router(
        TransportConfig {
            server_version: RELEASE.into(),
            ..config()
        },
        Registry::default(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let discover = app
        .clone()
        .oneshot(request(&modern("server/discover", json!({}))))
        .await
        .unwrap();
    let initialize = app
        .oneshot(request(&json!({"jsonrpc":"2.0", "id":0, "method":"initialize", "params":{"protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"legacy", "version":"1"}}})))
        .await
        .unwrap();
    for info in [
        json_response(discover).await["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]
            .clone(),
        json_response(initialize).await["result"]["serverInfo"].clone(),
    ] {
        assert_eq!("aura-historia-mcp", info["name"]);
        assert_eq!("Aura Historia", info["title"]);
        assert_eq!(RELEASE, info["version"]);
        assert_eq!("https://aura-historia.com", info["websiteUrl"]);
        assert!(!info["icons"].as_array().unwrap().is_empty());
    }
}

#[tokio::test]
async fn should_serve_exactly_the_configured_openai_apps_challenge() {
    let app = router(
        TransportConfig {
            openai_apps_challenge: Some("challenge-token_1".into()),
            ..config()
        },
        Registry::default(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    for method in ["GET", "HEAD"] {
        let response = app
            .clone()
            .oneshot(challenge_request(method, "mcp.example.test"))
            .await
            .unwrap();
        assert_eq!(StatusCode::OK, response.status(), "{method}");
        assert_eq!(
            "text/plain; charset=utf-8",
            response.headers()["content-type"]
        );
        assert_eq!("no-store", response.headers()["cache-control"]);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let expected: &[u8] = if method == "GET" {
            b"challenge-token_1"
        } else {
            b""
        };
        assert_eq!(expected, &body[..], "{method}");
    }
    let response = app
        .clone()
        .oneshot(challenge_request("GET", "attacker.test"))
        .await
        .unwrap();
    assert_eq!(StatusCode::FORBIDDEN, response.status());
    let mut foreign_origin = challenge_request("GET", "mcp.example.test");
    foreign_origin
        .headers_mut()
        .insert("origin", HeaderValue::from_static("https://attacker.test"));
    let response = app.oneshot(foreign_origin).await.unwrap();
    assert_eq!(StatusCode::FORBIDDEN, response.status());
}

#[tokio::test]
async fn should_not_find_the_openai_apps_challenge_without_a_token() {
    let app = router(config(), Registry::default(), Arc::new(AnonymousGate)).unwrap();
    let response = app
        .oneshot(challenge_request("GET", "mcp.example.test"))
        .await
        .unwrap();
    assert_eq!(StatusCode::NOT_FOUND, response.status());
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
}

#[test]
fn should_reject_an_invalid_release_or_challenge_configuration() {
    for version in [
        "0.1.0",
        "",
        "0123456789ABCDEF0123456789ABCDEF01234567",
        &RELEASE[1..],
    ] {
        let invalid = TransportConfig {
            server_version: version.into(),
            ..config()
        };
        assert!(
            router(invalid, Registry::default(), Arc::new(AnonymousGate)).is_err(),
            "{version:?}"
        );
    }
    let too_long = "x".repeat(513);
    for token in ["", "two words", "line\nbreak", "ünicode", too_long.as_str()] {
        let invalid = TransportConfig {
            openai_apps_challenge: Some(token.into()),
            ..config()
        };
        assert!(
            router(invalid, Registry::default(), Arc::new(AnonymousGate)).is_err(),
            "{token:?}"
        );
    }
}

#[test]
fn should_read_the_release_and_challenge_from_the_environment() {
    for (commit, challenge) in [
        (None, None),
        (Some(RELEASE), Some("challenge-token")),
        (Some("not-a-commit"), None),
        (None, Some("")),
    ] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "directory::release_configuration_probe",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("OAUTH_ISSUER_URL", "https://api.aura-historia.com");
        if let Some(commit) = commit {
            command.env("COMMIT_SHA", commit);
        }
        if let Some(challenge) = challenge {
            command.env("MCP_OPENAI_APPS_CHALLENGE", challenge);
        }
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
    }
}

#[test]
#[ignore = "invoked by the isolated environment configuration test"]
fn release_configuration_probe() {
    let commit = std::env::var("COMMIT_SHA").ok();
    let challenge = std::env::var("MCP_OPENAI_APPS_CHALLENGE").ok();
    let result = TransportConfig::from_env();
    match (commit.as_deref(), challenge.as_deref()) {
        (Some("not-a-commit"), _) => assert!(matches!(
            result,
            Err(aura_historia_mcp::ConfigError::ServerVersion)
        )),
        (_, Some("")) => assert!(matches!(
            result,
            Err(aura_historia_mcp::ConfigError::OpenAiAppsChallenge)
        )),
        _ => {
            let config = result.unwrap();
            assert_eq!(
                commit.as_deref().unwrap_or(UNRELEASED_SERVER_VERSION),
                config.server_version
            );
            assert_eq!(challenge, config.openai_apps_challenge);
        }
    }
}

fn challenge_request(method: &str, host: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri("/.well-known/openai-apps-challenge")
        .header("host", host)
        .body(Body::empty())
        .unwrap()
}
