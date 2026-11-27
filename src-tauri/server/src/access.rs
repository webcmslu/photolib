//! Who may use the server. With no `PHOTOTEK_PASSWORD`, everyone on the
//! network. With one, either:
//! - a browser, which asks for it itself (HTTP Basic, any user name); or
//! - an app (the desktop app, the mobile apps), which logs in once with
//!   `POST /api/login` and gets a token for its requests: in the
//!   `Authorization: Bearer` header, or as `?t=` on photo and thumbnail
//!   URLs (an `<img>` can't send headers). Tokens are kept in
//!   `<data>/sessions.json` and survive restarts; changing the password
//!   forgets them.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub struct Access {
    password: Option<String>,
    sessions: Mutex<HashSet<String>>,
    file: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    /// A hash of the password the tokens were given for.
    password: String,
    tokens: HashSet<String>,
}

fn hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

impl Access {
    pub fn new(password: Option<String>, data_dir: &std::path::Path) -> Self {
        let file = data_dir.join("sessions.json");
        let saved: Saved = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let current = hash(password.as_deref().unwrap_or(""));
        let sessions = if saved.password == current {
            saved.tokens
        } else {
            HashSet::new()
        };
        Self {
            password,
            sessions: Mutex::new(sessions),
            file,
        }
    }

    /// Compared through hashes, so the time taken says nothing about the password.
    fn password_matches(&self, given: &str) -> bool {
        self.password.as_deref().is_none_or(|password| {
            blake3::hash(given.as_bytes()) == blake3::hash(password.as_bytes())
        })
    }

    fn token_valid(&self, token: &str) -> bool {
        self.sessions.lock().unwrap().contains(&hash(token))
    }

    /// A new token, if the password is right.
    fn login(&self, given: &str) -> Option<String> {
        if !self.password_matches(given) {
            return None;
        }
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).ok()?;
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut sessions = self.sessions.lock().unwrap();
        // Only hashes are kept: the file alone lets nobody in.
        sessions.insert(hash(&token));
        let saved = Saved {
            password: hash(self.password.as_deref().unwrap_or("")),
            tokens: sessions.clone(),
        };
        if let Ok(json) = serde_json::to_vec(&saved) {
            let _ = std::fs::write(&self.file, json);
        }
        Some(token)
    }

    /// Whether a request may go through.
    fn allows(&self, request: &Request) -> bool {
        if self.password.is_none() {
            return true;
        }
        let authorization = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if let Some(token) = authorization.strip_prefix("Bearer ") {
            return self.token_valid(token);
        }
        if let Some(encoded) = authorization.strip_prefix("Basic ") {
            return base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
                .and_then(|decoded| String::from_utf8(decoded).ok())
                .and_then(|pair| {
                    pair.split_once(':')
                        .map(|(_, given)| self.password_matches(given))
                })
                .unwrap_or(false);
        }
        request
            .uri()
            .query()
            .unwrap_or("")
            .split('&')
            .find_map(|pair| pair.strip_prefix("t="))
            .is_some_and(|token| self.token_valid(token))
    }
}

#[derive(Deserialize)]
pub struct Login {
    #[serde(default)]
    password: String,
}

/// `POST /api/login` with `{ "password": "…" }`: `{ "token": "…" }`, or 401.
pub async fn login(State(access): State<Arc<Access>>, Json(login): Json<Login>) -> Response {
    match access.login(&login.password) {
        Some(token) => Json(json!({ "token": token })).into_response(),
        None => (StatusCode::UNAUTHORIZED, Json("Wrong password")).into_response(),
    }
}

pub async fn require(State(access): State<Arc<Access>>, request: Request, next: Next) -> Response {
    if request.uri().path() == "/api/login" || access.allows(&request) {
        return next.run(request).await;
    }
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    // Browsers then ask for the password; apps log in instead.
    if !request.uri().path().starts_with("/api/") {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Phototek\", charset=\"UTF-8\""),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(uri: &str, authorization: Option<&str>) -> Request {
        let mut builder = Request::builder().uri(uri);
        if let Some(value) = authorization {
            builder = builder.header(header::AUTHORIZATION, value);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn passwords_and_tokens() {
        let data = tempfile::tempdir().unwrap();
        let open = Access::new(None, data.path());
        assert!(open.allows(&request("/api/people", None)));

        let access = Access::new(Some("secret".into()), data.path());
        assert!(!access.allows(&request("/api/people", None)));
        assert!(access.login("wrong").is_none());
        let token = access.login("secret").unwrap();
        assert!(access.allows(&request("/api/people", Some(&format!("Bearer {token}")))));
        assert!(access.allows(&request(&format!("/thumb/grid/x?t={token}"), None)));
        assert!(!access.allows(&request("/api/people", Some("Bearer nope"))));
        // "x:secret", as a browser sends it.
        assert!(access.allows(&request("/", Some("Basic eDpzZWNyZXQ="))));

        // Tokens survive a restart, not a new password.
        let again = Access::new(Some("secret".into()), data.path());
        assert!(again.token_valid(&token));
        let changed = Access::new(Some("other".into()), data.path());
        assert!(!changed.token_valid(&token));
    }
}
