use super::{oauth, storage, IntegrationStatus};
use chrono::NaiveDate;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const AUTH_ENDPOINT: &str = "https://hackatime.hackclub.com/oauth/authorize";
const TOKEN_ENDPOINT: &str = "https://hackatime.hackclub.com/oauth/token";
const REVOKE_ENDPOINT: &str = "https://hackatime.hackclub.com/oauth/revoke";
const API_BASE: &str = "https://hackatime.hackclub.com/api/v1/authenticated";
const SCOPES: &str = "profile read";

#[derive(Default, Clone, Serialize, Deserialize)]
struct HackatimeAuth {
    client_id: Option<String>,
    access_token: Option<String>,
    expires_at: Option<i64>,
    account_label: Option<String>,
}

pub struct HackatimeState {
    auth: Mutex<HackatimeAuth>,
    path: PathBuf,
    http: reqwest::Client,
}

impl HackatimeState {
    pub fn load(dir: &Path) -> Self {
        Self {
            auth: Mutex::new(storage::load_or_default(&dir.join("hackatime_auth.json"))),
            path: dir.join("hackatime_auth.json"),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(25))
                .user_agent("Theta Hackatime integration")
                .build()
                .unwrap_or_default(),
        }
    }
    fn snapshot(&self) -> Result<HackatimeAuth, String> {
        self.auth
            .lock()
            .map(|a| a.clone())
            .map_err(|_| "Hackatime credentials are unavailable".into())
    }
    fn update(&self, f: impl FnOnce(&mut HackatimeAuth)) -> Result<(), String> {
        let mut auth = self
            .auth
            .lock()
            .map_err(|_| "Hackatime credentials are unavailable")?;
        f(&mut auth);
        storage::save_json(&self.path, &*auth, "Hackatime")
    }
    fn token(&self) -> Result<String, String> {
        let auth = self.snapshot()?;
        if auth
            .expires_at
            .is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp() + 60)
        {
            return Err("Hackatime authorization expired. Reconnect in Settings.".into());
        }
        auth.access_token
            .ok_or_else(|| "Hackatime is not connected".into())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
}

#[tauri::command]
pub fn hackatime_status(
    state: tauri::State<'_, HackatimeState>,
) -> Result<IntegrationStatus, String> {
    let auth = state.snapshot()?;
    let connected = auth.access_token.is_some()
        && !auth
            .expires_at
            .is_some_and(|value| value <= chrono::Utc::now().timestamp());
    Ok(IntegrationStatus {
        id: "hackatime",
        name: "Hackatime",
        configured: auth.client_id.is_some(),
        connected,
        account_label: auth.account_label,
        redirect_uri: Some(oauth::LOOPBACK_REDIRECT),
        message: Some("Read-only access to coding time, projects, streaks, and heartbeats.".into()),
    })
}

#[tauri::command]
pub fn hackatime_set_client_id(
    client_id: String,
    state: tauri::State<'_, HackatimeState>,
) -> Result<(), String> {
    let id = client_id.trim();
    if id.is_empty() || id.chars().any(char::is_whitespace) {
        return Err("Enter a valid Hackatime Client ID".into());
    }
    let changed = state.snapshot()?.client_id.as_deref() != Some(id);
    state.update(|auth| {
        auth.client_id = Some(id.into());
        if changed {
            auth.access_token = None;
            auth.expires_at = None;
            auth.account_label = None;
        }
    })
}

#[tauri::command]
pub async fn hackatime_disconnect(state: tauri::State<'_, HackatimeState>) -> Result<(), String> {
    if let Ok(token) = state.token() {
        let _ = state
            .http
            .post(REVOKE_ENDPOINT)
            .bearer_auth(token)
            .send()
            .await;
    }
    state.update(|auth| {
        auth.access_token = None;
        auth.expires_at = None;
        auth.account_label = None;
    })
}

#[tauri::command]
pub async fn hackatime_connect(state: tauri::State<'_, HackatimeState>) -> Result<String, String> {
    let client_id = state
        .snapshot()?
        .client_id
        .ok_or("Save a Hackatime Client ID first")?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Couldn't open the Hackatime callback port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let (verifier, challenge) = oauth::pkce_pair();
    let csrf = oauth::csrf_token();
    let url = format!("{AUTH_ENDPOINT}?client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge_method=S256&code_challenge={}&state={}", oauth::encode(&client_id), oauth::encode(&redirect_uri), oauth::encode(SCOPES), oauth::encode(&challenge), oauth::encode(&csrf));
    tauri_plugin_opener::open_url(&url, None::<&str>)
        .map_err(|e| format!("Couldn't open Hackatime sign-in: {e}"))?;
    let callback = tauri::async_runtime::spawn_blocking(move || {
        oauth::await_callback(&listener, 300, "Hackatime")
    })
    .await
    .map_err(|e| format!("Hackatime OAuth task failed: {e}"))??;
    if let Some(error) = callback.error {
        return Err(format!("Hackatime sign-in was denied: {error}"));
    }
    if callback.state.as_deref() != Some(&csrf) {
        return Err("Hackatime OAuth state mismatch — sign-in aborted".into());
    }
    let code = callback
        .code
        .ok_or("Hackatime did not return an authorization code")?;
    let response = state
        .http
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", client_id.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Hackatime token exchange failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Hackatime rejected sign-in ({status}). {}",
            oauth::clip_error(&text)
        ));
    }
    let token: TokenResponse = serde_json::from_str(&text)
        .map_err(|e| format!("Invalid Hackatime token response: {e}"))?;
    let expires_at = chrono::Utc::now().timestamp() + token.expires_in.unwrap_or(504_576_000);
    let profile_response = state
        .http
        .get(format!("{API_BASE}/me"))
        .bearer_auth(&token.access_token)
        .send()
        .await
        .map_err(|e| format!("Couldn't read Hackatime profile: {e}"))?;
    let profile: serde_json::Value = profile_response
        .json()
        .await
        .map_err(|e| format!("Invalid Hackatime profile response: {e}"))?;
    let label = profile
        .get("github_username")
        .and_then(|v| v.as_str())
        .or_else(|| {
            profile
                .get("emails")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
        })
        .unwrap_or("Hackatime account")
        .to_string();
    state.update(|auth| {
        auth.access_token = Some(token.access_token);
        auth.expires_at = Some(expires_at);
        auth.account_label = Some(label.clone());
    })?;
    Ok(label)
}

async fn api_call(
    state: &HackatimeState,
    method: Method,
    path: &str,
    query: &[(&str, String)],
) -> Result<serde_json::Value, String> {
    let response = state
        .http
        .request(method, format!("{API_BASE}{path}"))
        .bearer_auth(state.token()?)
        .query(query)
        .send()
        .await
        .map_err(|e| format!("Hackatime request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "Hackatime session expired. Reconnect in Settings.".into(),
            429 => "Hackatime rate limit reached. Try again shortly.".into(),
            _ => format!("Hackatime API error {status}: {}", oauth::clip_error(&text)),
        });
    }
    serde_json::from_str(&text).map_err(|e| format!("Invalid Hackatime response: {e}"))
}

fn valid_date(value: &str) -> bool {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

#[tauri::command]
pub async fn hackatime_get_profile(
    state: tauri::State<'_, HackatimeState>,
) -> Result<serde_json::Value, String> {
    api_call(&state, Method::GET, "/me", &[]).await
}
#[tauri::command]
pub async fn hackatime_get_streak(
    state: tauri::State<'_, HackatimeState>,
) -> Result<serde_json::Value, String> {
    api_call(&state, Method::GET, "/streak", &[]).await
}
#[tauri::command]
pub async fn hackatime_list_projects(
    include_archived: Option<bool>,
    state: tauri::State<'_, HackatimeState>,
) -> Result<serde_json::Value, String> {
    api_call(
        &state,
        Method::GET,
        "/projects",
        &[(
            "include_archived",
            include_archived.unwrap_or(false).to_string(),
        )],
    )
    .await
}
#[tauri::command]
pub async fn hackatime_latest_heartbeat(
    state: tauri::State<'_, HackatimeState>,
) -> Result<serde_json::Value, String> {
    api_call(&state, Method::GET, "/heartbeats/latest", &[]).await
}
#[tauri::command]
pub async fn hackatime_get_hours(
    start_date: Option<String>,
    end_date: Option<String>,
    state: tauri::State<'_, HackatimeState>,
) -> Result<serde_json::Value, String> {
    let mut query = vec![];
    if let Some(value) = start_date {
        if !valid_date(&value) {
            return Err("startDate must use YYYY-MM-DD".into());
        }
        query.push(("start_date", value));
    }
    if let Some(value) = end_date {
        if !valid_date(&value) {
            return Err("endDate must use YYYY-MM-DD".into());
        }
        query.push(("end_date", value));
    }
    api_call(&state, Method::GET, "/hours", &query).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_dates() {
        assert!(valid_date("2026-09-09"));
        assert!(!valid_date("09/09/2026"));
    }
}
