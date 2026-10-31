//! The browser sessions the review token opens: an in-memory session store and its cookie.

use super::ReviewServerConfig;
use crate::review::http::{HttpResponse, ParsedRequest};
use dashmap::DashMap;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const REVIEW_SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const REVIEW_SESSION_COOKIE: &str = "crawler_review_session";

/// The open browser sessions and when each expires; clones share the store.
#[derive(Clone, Default)]
pub(super) struct BrowserSessions {
    sessions: Arc<DashMap<uuid::Uuid, std::time::Instant>>,
}

impl BrowserSessions {
    /// Whether `headers` carry the cookie of an unexpired session. An expired session is removed.
    pub(super) fn is_valid(&self, headers: &std::collections::HashMap<String, String>) -> bool {
        let Some(session_id) = session_id_from_headers(headers) else {
            return false;
        };
        let now = std::time::Instant::now();
        match self.sessions.get(&session_id) {
            Some(expiry) if *expiry > now => true,
            Some(_) => {
                self.sessions.remove(&session_id);
                false
            }
            None => false,
        }
    }

    /// Opens a session, dropping expired ones, and answers with its cookie.
    pub(super) fn open(&self, config: &ReviewServerConfig) -> HttpResponse {
        let now = std::time::Instant::now();
        self.sessions.retain(|_, expiry| *expiry > now);
        let session_id = uuid::Uuid::new_v4();
        self.sessions.insert(session_id, now + REVIEW_SESSION_TTL);
        HttpResponse::json(200, &json!({ "ok": true })).with_header(
            "Set-Cookie",
            session_cookie(config, session_id, REVIEW_SESSION_TTL),
        )
    }

    /// Closes the request's session, if any, and answers with an expired cookie.
    pub(super) fn close(
        &self,
        config: &ReviewServerConfig,
        request: &ParsedRequest<'_>,
    ) -> HttpResponse {
        if let Some(session_id) = session_id_from_headers(&request.headers) {
            self.sessions.remove(&session_id);
        }
        HttpResponse::json(200, &json!({ "ok": true }))
            .with_header("Set-Cookie", expired_session_cookie(config))
    }
}

fn session_id_from_headers(
    headers: &std::collections::HashMap<String, String>,
) -> Option<uuid::Uuid> {
    headers
        .get("cookie")?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{REVIEW_SESSION_COOKIE}=")))
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
}

fn session_cookie(config: &ReviewServerConfig, session_id: uuid::Uuid, ttl: Duration) -> String {
    let secure = if config.bind_addr.ip().is_loopback() {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{REVIEW_SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        ttl.as_secs()
    )
}

fn expired_session_cookie(config: &ReviewServerConfig) -> String {
    session_cookie(config, uuid::Uuid::nil(), Duration::ZERO)
}
