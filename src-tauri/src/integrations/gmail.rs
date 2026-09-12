use super::{oauth, storage, IntegrationStatus};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const API_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const IDENTITY_SCOPE: &str = "https://www.googleapis.com/auth/userinfo.email";
const READ_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
const SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";
const COMPOSE_SCOPE: &str = "https://www.googleapis.com/auth/gmail.compose";
const MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";

#[derive(Default, Clone, Serialize, Deserialize)]
struct GmailAuth {
    client_id: Option<String>,
    refresh_token: Option<String>,
    access_token: Option<String>,
    expires_at: Option<i64>,
    account_label: Option<String>,
    scopes: Vec<String>,
}
pub struct GmailState {
    auth: Mutex<GmailAuth>,
    path: PathBuf,
    http: reqwest::Client,
}
impl GmailState {
    pub fn load(dir: &Path) -> Self {
        Self {
            auth: Mutex::new(storage::load_or_default(&dir.join("gmail_auth.json"))),
            path: dir.join("gmail_auth.json"),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("Theta Gmail integration")
                .build()
                .unwrap_or_default(),
        }
    }
    fn snapshot(&self) -> Result<GmailAuth, String> {
        self.auth
            .lock()
            .map(|a| a.clone())
            .map_err(|_| "Gmail credentials are unavailable".into())
    }
    fn update(&self, f: impl FnOnce(&mut GmailAuth)) -> Result<(), String> {
        let mut a = self
            .auth
            .lock()
            .map_err(|_| "Gmail credentials are unavailable")?;
        f(&mut a);
        storage::save_json(&self.path, &*a, "Gmail")
    }
    async fn token(&self) -> Result<String, String> {
        let a = self.snapshot()?;
        let now = chrono::Utc::now().timestamp();
        if let (Some(token), Some(expiry)) = (&a.access_token, a.expires_at) {
            if expiry > now + 120 {
                return Ok(token.clone());
            }
        }
        let client = a.client_id.ok_or("Gmail is not configured")?;
        let refresh = a.refresh_token.ok_or("Gmail is not connected")?;
        let response = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("client_id", client.as_str()),
                ("refresh_token", refresh.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| format!("Gmail token refresh failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "Gmail rejected token refresh ({status}). {}",
                oauth::clip_error(&text)
            ));
        }
        let token: TokenResponse = serde_json::from_str(&text)
            .map_err(|e| format!("Invalid Gmail token response: {e}"))?;
        let access = token.access_token.clone();
        self.update(|a| {
            a.access_token = Some(token.access_token);
            a.expires_at = Some(now + token.expires_in.unwrap_or(3600));
            if token.refresh_token.is_some() {
                a.refresh_token = token.refresh_token
            }
        })?;
        Ok(access)
    }
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}
#[derive(Deserialize)]
struct Profile {
    email_address: String,
}
fn scopes(capabilities: &[String]) -> Vec<String> {
    let mut out = vec![IDENTITY_SCOPE.into(), READ_SCOPE.into()];
    for c in capabilities {
        match c.as_str() {
            "compose" => out.push(COMPOSE_SCOPE.into()),
            "send" => out.push(SEND_SCOPE.into()),
            "modify" => out.push(MODIFY_SCOPE.into()),
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

#[tauri::command]
pub fn gmail_status(state: tauri::State<'_, GmailState>) -> Result<IntegrationStatus, String> {
    let a = state.snapshot()?;
    Ok(IntegrationStatus {
        id: "gmail",
        name: "Gmail",
        configured: a.client_id.is_some(),
        connected: a.refresh_token.is_some(),
        account_label: a.account_label,
        redirect_uri: Some(oauth::LOOPBACK_REDIRECT),
        message: Some(if a.scopes.is_empty() {
            "Read-only by default; request compose, send, or modify access explicitly.".into()
        } else {
            format!("Granted {} Gmail scopes.", a.scopes.len())
        }),
    })
}
#[tauri::command]
pub fn gmail_set_client_id(
    client_id: String,
    state: tauri::State<'_, GmailState>,
) -> Result<(), String> {
    let id = client_id.trim();
    if id.is_empty() || id.chars().any(char::is_whitespace) {
        return Err("Enter a valid Google Desktop OAuth Client ID".into());
    }
    let changed = state.snapshot()?.client_id.as_deref() != Some(id);
    state.update(|a| {
        a.client_id = Some(id.into());
        if changed {
            a.refresh_token = None;
            a.access_token = None;
            a.expires_at = None;
            a.account_label = None;
            a.scopes.clear()
        }
    })
}
#[tauri::command]
pub fn gmail_disconnect(state: tauri::State<'_, GmailState>) -> Result<(), String> {
    state.update(|a| {
        a.refresh_token = None;
        a.access_token = None;
        a.expires_at = None;
        a.account_label = None;
        a.scopes.clear()
    })
}
#[tauri::command]
pub async fn gmail_connect(
    capabilities: Option<Vec<String>>,
    state: tauri::State<'_, GmailState>,
) -> Result<String, String> {
    let client = state
        .snapshot()?
        .client_id
        .ok_or("Save a Google Desktop OAuth Client ID first")?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Couldn't open the Gmail callback port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let requested = scopes(&capabilities.unwrap_or_default());
    let scope = requested.join(" ");
    let (verifier, challenge) = oauth::pkce_pair();
    let csrf = oauth::csrf_token();
    let url=format!("{AUTH_ENDPOINT}?client_id={}&response_type=code&redirect_uri={}&scope={}&access_type=offline&prompt=consent&include_granted_scopes=true&code_challenge_method=S256&code_challenge={}&state={}",oauth::encode(&client),oauth::encode(&redirect),oauth::encode(&scope),oauth::encode(&challenge),oauth::encode(&csrf));
    tauri_plugin_opener::open_url(&url, None::<&str>)
        .map_err(|e| format!("Couldn't open Gmail sign-in: {e}"))?;
    let callback = tauri::async_runtime::spawn_blocking(move || {
        oauth::await_callback(&listener, 300, "Gmail")
    })
    .await
    .map_err(|e| format!("Gmail OAuth task failed: {e}"))??;
    if let Some(error) = callback.error {
        return Err(format!("Gmail sign-in was denied: {error}"));
    }
    if callback.state.as_deref() != Some(&csrf) {
        return Err("Gmail OAuth state mismatch — sign-in aborted".into());
    }
    let code = callback
        .code
        .ok_or("Google did not return an authorization code")?;
    let response = state
        .http
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", client.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Gmail token exchange failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Google rejected Gmail sign-in ({status}). {}",
            oauth::clip_error(&text)
        ));
    }
    let token: TokenResponse =
        serde_json::from_str(&text).map_err(|e| format!("Invalid Gmail token response: {e}"))?;
    let refresh = token
        .refresh_token
        .ok_or("Google did not issue a refresh token. Revoke Theta access and reconnect.")?;
    let access = token.access_token;
    let profile = state
        .http
        .get(format!("{API_BASE}/profile"))
        .bearer_auth(&access)
        .send()
        .await
        .map_err(|e| format!("Couldn't read Gmail profile: {e}"))?
        .json::<Profile>()
        .await
        .map_err(|e| format!("Invalid Gmail profile: {e}"))?;
    let granted = token
        .scope
        .unwrap_or(scope)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let expiry = chrono::Utc::now().timestamp() + token.expires_in.unwrap_or(3600);
    let label = profile.email_address;
    state.update(|a| {
        a.refresh_token = Some(refresh);
        a.access_token = Some(access);
        a.expires_at = Some(expiry);
        a.account_label = Some(label.clone());
        a.scopes = granted
    })?;
    Ok(label)
}
fn has_scope(a: &GmailAuth, scope: &str) -> bool {
    a.scopes.iter().any(|s| s == scope)
}
async fn api(
    state: &GmailState,
    method: reqwest::Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let mut req = state
        .http
        .request(method, format!("{API_BASE}{path}"))
        .bearer_auth(state.token().await?)
        .query(query);
    if let Some(v) = body {
        req = req.json(&v)
    }
    let response = req
        .send()
        .await
        .map_err(|e| format!("Gmail request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "Gmail session expired. Reconnect in Settings.".into(),
            403 => format!(
                "Gmail scope or permission error: {}",
                oauth::clip_error(&text)
            ),
            429 => "Gmail rate limit reached. Try again shortly.".into(),
            _ => format!("Gmail API error {status}: {}", oauth::clip_error(&text)),
        });
    }
    if text.trim().is_empty() {
        Ok(serde_json::json!({"ok":true}))
    } else {
        serde_json::from_str(&text).map_err(|e| format!("Invalid Gmail response: {e}"))
    }
}
#[tauri::command]
pub async fn gmail_get_profile(
    state: tauri::State<'_, GmailState>,
) -> Result<serde_json::Value, String> {
    api(&state, reqwest::Method::GET, "/profile", &[], None).await
}
#[tauri::command]
pub async fn gmail_search_messages(
    query: String,
    max_results: Option<u8>,
    state: tauri::State<'_, GmailState>,
) -> Result<serde_json::Value, String> {
    api(
        &state,
        reqwest::Method::GET,
        "/messages",
        &[
            ("q", query),
            (
                "maxResults",
                max_results.unwrap_or(20).clamp(1, 100).to_string(),
            ),
        ],
        None,
    )
    .await
}
#[tauri::command]
pub async fn gmail_get_message(
    message_id: String,
    state: tauri::State<'_, GmailState>,
) -> Result<serde_json::Value, String> {
    api(
        &state,
        reqwest::Method::GET,
        &format!("/messages/{message_id}"),
        &[("format", "metadata".into())],
        None,
    )
    .await
}
#[tauri::command]
pub async fn gmail_get_thread(
    thread_id: String,
    state: tauri::State<'_, GmailState>,
) -> Result<serde_json::Value, String> {
    api(
        &state,
        reqwest::Method::GET,
        &format!("/threads/{thread_id}"),
        &[("format", "metadata".into())],
        None,
    )
    .await
}
fn message_raw(to: &str, subject: &str, body: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(
        "To: {to}\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}"
    ))
}
#[tauri::command]
pub async fn gmail_create_draft(
    to: String,
    subject: String,
    body: String,
    state: tauri::State<'_, GmailState>,
) -> Result<serde_json::Value, String> {
    if !has_scope(&state.snapshot()?, COMPOSE_SCOPE) {
        return Err("Reconnect Gmail with compose access enabled.".into());
    }
    api(
        &state,
        reqwest::Method::POST,
        "/drafts",
        &[],
        Some(serde_json::json!({"message":{"raw":message_raw(&to,&subject,&body)}})),
    )
    .await
}
#[tauri::command]
pub async fn gmail_send_message(
    to: String,
    subject: String,
    body: String,
    state: tauri::State<'_, GmailState>,
) -> Result<serde_json::Value, String> {
    if !has_scope(&state.snapshot()?, SEND_SCOPE) {
        return Err("Reconnect Gmail with send access enabled.".into());
    }
    api(
        &state,
        reqwest::Method::POST,
        "/messages/send",
        &[],
        Some(serde_json::json!({"raw":message_raw(&to,&subject,&body)})),
    )
    .await
}
#[tauri::command]
pub async fn gmail_modify_message(
    message_id: String,
    add_label_ids: Vec<String>,
    remove_label_ids: Vec<String>,
    state: tauri::State<'_, GmailState>,
) -> Result<serde_json::Value, String> {
    if !has_scope(&state.snapshot()?, MODIFY_SCOPE) {
        return Err("Reconnect Gmail with modify access enabled.".into());
    }
    api(
        &state,
        reqwest::Method::POST,
        &format!("/messages/{message_id}/modify"),
        &[],
        Some(serde_json::json!({"addLabelIds":add_label_ids,"removeLabelIds":remove_label_ids})),
    )
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_to_read_only() {
        let got = scopes(&[]);
        assert!(got.contains(&READ_SCOPE.into()));
        assert!(!got.contains(&SEND_SCOPE.into()));
    }
}
