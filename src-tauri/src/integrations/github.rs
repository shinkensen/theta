use super::{oauth, storage, IntegrationStatus};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const DEVICE_ENDPOINT: &str = "https://github.com/login/device/code";
const TOKEN_ENDPOINT: &str = "https://github.com/login/oauth/access_token";
const API_BASE: &str = "https://api.github.com";
const SCOPES: &str = "repo notifications read:user user:email";

#[derive(Default, Clone, Serialize, Deserialize)]
struct GithubAuth {
    client_id: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
    refresh_expires_at: Option<i64>,
    account_label: Option<String>,
    scopes: Vec<String>,
}
#[derive(Clone)]
struct PendingDevice {
    device_code: String,
    expires_at: i64,
    interval: u64,
}

pub struct GithubState {
    auth: Mutex<GithubAuth>,
    pending: Mutex<Option<PendingDevice>>,
    path: PathBuf,
    http: reqwest::Client,
}
impl GithubState {
    pub fn load(dir: &Path) -> Self {
        Self {
            auth: Mutex::new(storage::load_or_default(&dir.join("github_auth.json"))),
            pending: Mutex::new(None),
            path: dir.join("github_auth.json"),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(25))
                .user_agent("Theta GitHub integration")
                .build()
                .unwrap_or_default(),
        }
    }
    fn snapshot(&self) -> Result<GithubAuth, String> {
        self.auth
            .lock()
            .map(|a| a.clone())
            .map_err(|_| "GitHub credentials are unavailable".into())
    }
    fn update(&self, f: impl FnOnce(&mut GithubAuth)) -> Result<(), String> {
        let mut auth = self
            .auth
            .lock()
            .map_err(|_| "GitHub credentials are unavailable")?;
        f(&mut auth);
        storage::save_json(&self.path, &*auth, "GitHub")
    }
    async fn token(&self) -> Result<String, String> {
        let auth = self.snapshot()?;
        if !auth
            .expires_at
            .is_some_and(|v| v <= chrono::Utc::now().timestamp() + 120)
        {
            return auth
                .access_token
                .ok_or_else(|| "GitHub is not connected".into());
        }
        let refresh = auth
            .refresh_token
            .ok_or("GitHub session expired. Reconnect in Settings.")?;
        let client = auth.client_id.ok_or("GitHub is not configured")?;
        let response = self
            .http
            .post(TOKEN_ENDPOINT)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("GitHub token refresh failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "GitHub rejected token refresh ({status}). {}",
                oauth::clip_error(&text)
            ));
        }
        let token: TokenResponse = serde_json::from_str(&text)
            .map_err(|e| format!("Invalid GitHub token response: {e}"))?;
        let access = token.access_token.clone();
        let now = chrono::Utc::now().timestamp();
        self.update(|a| {
            a.access_token = Some(token.access_token);
            if token.refresh_token.is_some() {
                a.refresh_token = token.refresh_token;
            }
            a.expires_at = token.expires_in.map(|v| now + v);
            a.refresh_expires_at = token.refresh_token_expires_in.map(|v| now + v);
        })?;
        Ok(access)
    }
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: i64,
    interval: Option<u64>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
}
#[derive(Deserialize)]
struct PollResponse {
    access_token: Option<String>,
    scope: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorization {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: i64,
    pub interval_seconds: u64,
}

#[tauri::command]
pub fn github_status(state: tauri::State<'_, GithubState>) -> Result<IntegrationStatus, String> {
    let auth = state.snapshot()?;
    Ok(IntegrationStatus {
        id: "github",
        name: "GitHub",
        configured: auth.client_id.is_some(),
        connected: auth.access_token.is_some(),
        account_label: auth.account_label,
        redirect_uri: None,
        message: Some(if auth.scopes.is_empty() {
            "Uses GitHub's secretless device authorization flow.".into()
        } else {
            format!("Granted scopes: {}", auth.scopes.join(", "))
        }),
    })
}
#[tauri::command]
pub fn github_set_client_id(
    client_id: String,
    state: tauri::State<'_, GithubState>,
) -> Result<(), String> {
    let id = client_id.trim();
    if id.is_empty() || id.chars().any(char::is_whitespace) {
        return Err("Enter a valid GitHub Client ID".into());
    }
    let changed = state.snapshot()?.client_id.as_deref() != Some(id);
    state.update(|a| {
        a.client_id = Some(id.into());
        if changed {
            a.access_token = None;
            a.refresh_token = None;
            a.account_label = None;
            a.scopes.clear();
        }
    })
}
#[tauri::command]
pub fn github_disconnect(state: tauri::State<'_, GithubState>) -> Result<(), String> {
    state.update(|a| {
        a.access_token = None;
        a.refresh_token = None;
        a.expires_at = None;
        a.refresh_expires_at = None;
        a.account_label = None;
        a.scopes.clear();
    })
}

#[tauri::command]
pub async fn github_begin_device_flow(
    state: tauri::State<'_, GithubState>,
) -> Result<DeviceAuthorization, String> {
    let client = state
        .snapshot()?
        .client_id
        .ok_or("Save a GitHub Client ID first")?;
    let response = state
        .http
        .post(DEVICE_ENDPOINT)
        .header("Accept", "application/json")
        .form(&[("client_id", client.as_str()), ("scope", SCOPES)])
        .send()
        .await
        .map_err(|e| format!("GitHub device authorization failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "GitHub rejected device authorization ({status}). {}",
            oauth::clip_error(&text)
        ));
    }
    let device: DeviceResponse =
        serde_json::from_str(&text).map_err(|e| format!("Invalid GitHub device response: {e}"))?;
    let interval = device.interval.unwrap_or(5).max(1);
    let expires_at = chrono::Utc::now().timestamp() + device.expires_in;
    *state
        .pending
        .lock()
        .map_err(|_| "GitHub authorization state is unavailable")? = Some(PendingDevice {
        device_code: device.device_code,
        expires_at,
        interval,
    });
    Ok(DeviceAuthorization {
        user_code: device.user_code,
        verification_uri: device.verification_uri,
        expires_at,
        interval_seconds: interval,
    })
}

#[tauri::command]
pub async fn github_poll_device_flow(
    state: tauri::State<'_, GithubState>,
) -> Result<String, String> {
    loop {
        let pending = state
            .pending
            .lock()
            .map_err(|_| "GitHub authorization state is unavailable")?
            .clone()
            .ok_or("Start GitHub device authorization first")?;
        if chrono::Utc::now().timestamp() >= pending.expires_at {
            return Err("GitHub device code expired. Start again.".into());
        }
        tokio::time::sleep(Duration::from_secs(pending.interval)).await;
        let client = state
            .snapshot()?
            .client_id
            .ok_or("GitHub is not configured")?;
        let response = state
            .http
            .post(TOKEN_ENDPOINT)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client.as_str()),
                ("device_code", pending.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|e| format!("GitHub device polling failed: {e}"))?;
        let body: PollResponse = response
            .json()
            .await
            .map_err(|e| format!("Invalid GitHub token response: {e}"))?;
        if let Some(access) = body.access_token {
            let now = chrono::Utc::now().timestamp();
            let profile = state
                .http
                .get(format!("{API_BASE}/user"))
                .bearer_auth(&access)
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .map_err(|e| format!("Couldn't read GitHub profile: {e}"))?
                .json::<serde_json::Value>()
                .await
                .map_err(|e| format!("Invalid GitHub profile: {e}"))?;
            let label = profile
                .get("login")
                .and_then(|v| v.as_str())
                .unwrap_or("GitHub account")
                .to_string();
            let scopes = body
                .scope
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect();
            state.update(|a| {
                a.access_token = Some(access);
                a.refresh_token = body.refresh_token;
                a.expires_at = body.expires_in.map(|v| now + v);
                a.refresh_expires_at = body.refresh_token_expires_in.map(|v| now + v);
                a.scopes = scopes;
                a.account_label = Some(label.clone());
            })?;
            *state
                .pending
                .lock()
                .map_err(|_| "GitHub authorization state is unavailable")? = None;
            return Ok(label);
        }
        match body.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                if let Ok(mut guard) = state.pending.lock() {
                    if let Some(p) = guard.as_mut() {
                        p.interval = body.interval.unwrap_or(p.interval + 5).max(p.interval + 5);
                    }
                }
            }
            Some("access_denied") => return Err("GitHub authorization was denied.".into()),
            Some("expired_token") => return Err("GitHub device code expired. Start again.".into()),
            Some(error) => {
                return Err(format!(
                    "GitHub authorization failed: {}",
                    body.error_description.unwrap_or_else(|| error.into())
                ))
            }
            None => return Err("GitHub returned no access token.".into()),
        }
    }
}

async fn api(
    state: &GithubState,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let mut request = state
        .http
        .request(method, format!("{API_BASE}{path}"))
        .bearer_auth(state.token().await?)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .query(query);
    if let Some(value) = body {
        request = request.json(&value)
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("GitHub request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "GitHub session expired. Reconnect in Settings.".into(),
            403 | 429 => format!(
                "GitHub rate limit or permission error: {}",
                oauth::clip_error(&text)
            ),
            _ => format!("GitHub API error {status}: {}", oauth::clip_error(&text)),
        });
    }
    if text.trim().is_empty() {
        Ok(serde_json::json!({"ok":true}))
    } else {
        serde_json::from_str(&text).map_err(|e| format!("Invalid GitHub response: {e}"))
    }
}
fn limit(value: Option<u8>) -> String {
    value.unwrap_or(30).clamp(1, 100).to_string()
}
#[tauri::command]
pub async fn github_get_profile(
    state: tauri::State<'_, GithubState>,
) -> Result<serde_json::Value, String> {
    api(&state, Method::GET, "/user", &[], None).await
}
#[tauri::command]
pub async fn github_list_repositories(
    per_page: Option<u8>,
    state: tauri::State<'_, GithubState>,
) -> Result<serde_json::Value, String> {
    api(
        &state,
        Method::GET,
        "/user/repos",
        &[("per_page", limit(per_page)), ("sort", "updated".into())],
        None,
    )
    .await
}
#[tauri::command]
pub async fn github_list_notifications(
    per_page: Option<u8>,
    state: tauri::State<'_, GithubState>,
) -> Result<serde_json::Value, String> {
    api(
        &state,
        Method::GET,
        "/notifications",
        &[("per_page", limit(per_page))],
        None,
    )
    .await
}
#[tauri::command]
pub async fn github_search_issues(
    query: String,
    per_page: Option<u8>,
    state: tauri::State<'_, GithubState>,
) -> Result<serde_json::Value, String> {
    if query.trim().is_empty() {
        return Err("Search query is required".into());
    }
    api(
        &state,
        Method::GET,
        "/search/issues",
        &[("q", query), ("per_page", limit(per_page))],
        None,
    )
    .await
}
#[tauri::command]
pub async fn github_create_issue(
    owner: String,
    repo: String,
    title: String,
    body: Option<String>,
    state: tauri::State<'_, GithubState>,
) -> Result<serde_json::Value, String> {
    api(
        &state,
        Method::POST,
        &format!("/repos/{owner}/{repo}/issues"),
        &[],
        Some(serde_json::json!({"title":title,"body":body})),
    )
    .await
}
#[tauri::command]
pub async fn github_comment_issue(
    owner: String,
    repo: String,
    issue_number: u64,
    body: String,
    state: tauri::State<'_, GithubState>,
) -> Result<serde_json::Value, String> {
    api(
        &state,
        Method::POST,
        &format!("/repos/{owner}/{repo}/issues/{issue_number}/comments"),
        &[],
        Some(serde_json::json!({"body":body})),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamps_page_size() {
        assert_eq!(limit(Some(0)), "1");
        assert_eq!(limit(Some(200)), "100");
    }
}
