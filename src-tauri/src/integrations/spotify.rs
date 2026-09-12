use super::{oauth, storage, IntegrationStatus};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const AUTH_ENDPOINT: &str = "https://accounts.spotify.com/authorize";
const TOKEN_ENDPOINT: &str = "https://accounts.spotify.com/api/token";
const API_BASE: &str = "https://api.spotify.com/v1";
const SCOPES: &str = "user-read-playback-state user-modify-playback-state";
const REFRESH_SKEW_SECS: i64 = 120;
const CONSENT_TIMEOUT_SECS: u64 = 300;

#[derive(Default, Clone, Serialize, Deserialize)]
struct SpotifyAuth {
    client_id: Option<String>,
    refresh_token: Option<String>,
    access_token: Option<String>,
    expires_at: Option<i64>,
    account_label: Option<String>,
}

pub struct SpotifyState {
    auth: Mutex<SpotifyAuth>,
    path: PathBuf,
    http: reqwest::Client,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyDevice {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub is_active: bool,
    pub is_restricted: bool,
    pub volume_percent: Option<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyTrack {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
    pub artwork_url: Option<String>,
    pub duration_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyPlayback {
    pub active: bool,
    pub is_playing: bool,
    pub progress_ms: u64,
    pub repeat_state: Option<String>,
    pub shuffle_state: Option<bool>,
    pub device: Option<SpotifyDevice>,
    pub track: Option<SpotifyTrack>,
}

impl SpotifyState {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("spotify_auth.json");
        let auth = storage::load_or_default(&path);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .user_agent("Theta Spotify integration")
            .build()
            .unwrap_or_default();
        Self {
            auth: Mutex::new(auth),
            path,
            http,
        }
    }

    fn snapshot(&self) -> Result<SpotifyAuth, String> {
        self.auth
            .lock()
            .map(|auth| auth.clone())
            .map_err(|_| "Spotify credentials are unavailable".into())
    }

    fn update(&self, f: impl FnOnce(&mut SpotifyAuth)) -> Result<SpotifyAuth, String> {
        let mut guard = self
            .auth
            .lock()
            .map_err(|_| "Spotify credentials are unavailable")?;
        f(&mut guard);
        let snapshot = guard.clone();
        storage::save_json(&self.path, &snapshot, "Spotify")?;
        Ok(snapshot)
    }

    async fn access_token(&self) -> Result<String, String> {
        let auth = self.snapshot()?;
        let now = chrono::Utc::now().timestamp();
        if let (Some(token), Some(expiry)) = (&auth.access_token, auth.expires_at) {
            if expiry - REFRESH_SKEW_SECS > now {
                return Ok(token.clone());
            }
        }
        let client_id = auth.client_id.ok_or("Spotify is not configured")?;
        let refresh_token = auth.refresh_token.ok_or("Spotify is not connected")?;
        let response = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", client_id.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("Spotify token refresh failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "Spotify rejected the token refresh ({status}). Reconnect in Settings. {}",
                oauth::clip_error(&text)
            ));
        }
        let token: TokenResponse = serde_json::from_str(&text)
            .map_err(|e| format!("Invalid Spotify token response: {e}"))?;
        let access = token.access_token.clone();
        self.update(|auth| {
            auth.access_token = Some(token.access_token);
            auth.expires_at = Some(now + token.expires_in.unwrap_or(3600));
            if token.refresh_token.is_some() {
                auth.refresh_token = token.refresh_token;
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
}

#[tauri::command]
pub fn spotify_status(state: tauri::State<'_, SpotifyState>) -> Result<IntegrationStatus, String> {
    let auth = state.snapshot()?;
    Ok(IntegrationStatus {
        id: "spotify",
        name: "Spotify",
        configured: auth.client_id.is_some(),
        connected: auth.refresh_token.is_some(),
        account_label: auth.account_label,
        redirect_uri: Some(oauth::LOOPBACK_REDIRECT),
        message: None,
    })
}

#[tauri::command]
pub fn spotify_set_client_id(
    client_id: String,
    state: tauri::State<'_, SpotifyState>,
) -> Result<(), String> {
    let client_id = client_id.trim();
    if client_id.is_empty() || client_id.chars().any(char::is_whitespace) {
        return Err("Enter a valid Spotify Client ID".into());
    }
    let changed = state.snapshot()?.client_id.as_deref() != Some(client_id);
    state.update(|auth| {
        auth.client_id = Some(client_id.to_owned());
        if changed {
            auth.refresh_token = None;
            auth.access_token = None;
            auth.expires_at = None;
            auth.account_label = None;
        }
    })?;
    Ok(())
}

#[tauri::command]
pub fn spotify_disconnect(state: tauri::State<'_, SpotifyState>) -> Result<(), String> {
    state.update(|auth| {
        auth.refresh_token = None;
        auth.access_token = None;
        auth.expires_at = None;
        auth.account_label = None;
    })?;
    Ok(())
}

#[tauri::command]
pub async fn spotify_connect(state: tauri::State<'_, SpotifyState>) -> Result<String, String> {
    let client_id = state
        .snapshot()?
        .client_id
        .ok_or("Save a Spotify Client ID first")?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Couldn't open the Spotify callback port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let (verifier, challenge) = oauth::pkce_pair();
    let csrf = oauth::csrf_token();
    let auth_url = format!(
        "{AUTH_ENDPOINT}?client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge_method=S256&code_challenge={}&state={}",
        oauth::encode(&client_id), oauth::encode(&redirect_uri), oauth::encode(SCOPES), oauth::encode(&challenge), oauth::encode(&csrf)
    );
    tauri_plugin_opener::open_url(&auth_url, None::<&str>)
        .map_err(|e| format!("Couldn't open Spotify sign-in: {e}"))?;
    let callback = tauri::async_runtime::spawn_blocking(move || {
        oauth::await_callback(&listener, CONSENT_TIMEOUT_SECS, "Spotify")
    })
    .await
    .map_err(|e| format!("Spotify OAuth task failed: {e}"))??;
    if let Some(error) = callback.error {
        return Err(format!("Spotify sign-in was denied: {error}"));
    }
    if callback.state.as_deref() != Some(csrf.as_str()) {
        return Err("Spotify OAuth state mismatch — sign-in aborted".into());
    }
    let code = callback
        .code
        .ok_or("Spotify did not return an authorization code")?;
    let response = state
        .http
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Spotify token exchange failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Spotify rejected the token exchange ({status}). {}",
            oauth::clip_error(&text)
        ));
    }
    let token: TokenResponse =
        serde_json::from_str(&text).map_err(|e| format!("Invalid Spotify token response: {e}"))?;
    let refresh = token
        .refresh_token
        .ok_or("Spotify did not issue a refresh token")?;
    let expires_at = chrono::Utc::now().timestamp() + token.expires_in.unwrap_or(3600);
    let access = token.access_token;
    let profile = state
        .http
        .get(format!("{API_BASE}/me"))
        .bearer_auth(&access)
        .send()
        .await
        .map_err(|e| format!("Couldn't read Spotify profile: {e}"))?;
    let profile_json: serde_json::Value = profile.json().await.unwrap_or_default();
    let label = profile_json
        .get("display_name")
        .and_then(|value| value.as_str())
        .or_else(|| profile_json.get("id").and_then(|value| value.as_str()))
        .unwrap_or("Spotify account")
        .to_string();
    state.update(|auth| {
        auth.access_token = Some(access);
        auth.refresh_token = Some(refresh);
        auth.expires_at = Some(expires_at);
        auth.account_label = Some(label.clone());
    })?;
    Ok(label)
}

async fn api_call(
    state: &SpotifyState,
    method: Method,
    path: &str,
    query: &[(&str, String)],
) -> Result<Option<serde_json::Value>, String> {
    let token = state.access_token().await?;
    let response = state
        .http
        .request(method, format!("{API_BASE}{path}"))
        .bearer_auth(token)
        .query(query)
        .send()
        .await
        .map_err(|e| format!("Spotify request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status == StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if !status.is_success() {
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|body| {
                body.pointer("/error/message")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| oauth::clip_error(&text));
        return Err(match status {
            StatusCode::UNAUTHORIZED => "Spotify session expired. Reconnect in Settings.".into(),
            StatusCode::FORBIDDEN => format!(
                "Spotify denied playback control. A Premium account may be required. {detail}"
            ),
            StatusCode::NOT_FOUND => {
                "No active Spotify device was found. Start Spotify on a device first.".into()
            }
            StatusCode::TOO_MANY_REQUESTS => {
                "Spotify rate limit reached. Try again shortly.".into()
            }
            _ => format!("Spotify API error {status}: {detail}"),
        });
    }
    if text.trim().is_empty() {
        Ok(None)
    } else {
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("Invalid Spotify response: {e}"))
    }
}

fn parse_device(value: &serde_json::Value) -> Option<SpotifyDevice> {
    Some(SpotifyDevice {
        id: value.get("id")?.as_str()?.to_owned(),
        name: value.get("name")?.as_str()?.to_owned(),
        device_type: value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_owned(),
        is_active: value
            .get("is_active")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        is_restricted: value
            .get("is_restricted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        volume_percent: value
            .get("volume_percent")
            .and_then(|v| v.as_u64())
            .map(|v| v.min(100) as u8),
    })
}

fn parse_playback(value: Option<serde_json::Value>) -> SpotifyPlayback {
    let Some(value) = value else {
        return SpotifyPlayback {
            active: false,
            is_playing: false,
            progress_ms: 0,
            repeat_state: None,
            shuffle_state: None,
            device: None,
            track: None,
        };
    };
    let track = value.get("item").and_then(|item| {
        Some(SpotifyTrack {
            uri: item.get("uri")?.as_str()?.to_owned(),
            name: item.get("name")?.as_str()?.to_owned(),
            artists: item
                .get("artists")
                .and_then(|v| v.as_array())
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| row.get("name")?.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            album: item
                .pointer("/album/name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
            artwork_url: item
                .pointer("/album/images/0/url")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            duration_ms: item
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        })
    });
    SpotifyPlayback {
        active: true,
        is_playing: value
            .get("is_playing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        progress_ms: value
            .get("progress_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        repeat_state: value
            .get("repeat_state")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        shuffle_state: value.get("shuffle_state").and_then(|v| v.as_bool()),
        device: value.get("device").and_then(parse_device),
        track,
    }
}

#[tauri::command]
pub async fn spotify_get_playback(
    state: tauri::State<'_, SpotifyState>,
) -> Result<SpotifyPlayback, String> {
    Ok(parse_playback(
        api_call(&state, Method::GET, "/me/player", &[]).await?,
    ))
}
#[tauri::command]
pub async fn spotify_list_devices(
    state: tauri::State<'_, SpotifyState>,
) -> Result<Vec<SpotifyDevice>, String> {
    let body = api_call(&state, Method::GET, "/me/player/devices", &[])
        .await?
        .unwrap_or_default();
    Ok(body
        .get("devices")
        .and_then(|v| v.as_array())
        .map(|rows| rows.iter().filter_map(parse_device).collect())
        .unwrap_or_default())
}

async fn control(
    state: &SpotifyState,
    method: Method,
    path: &str,
    device_id: Option<String>,
    extra: Vec<(&str, String)>,
) -> Result<(), String> {
    let mut query = extra;
    if let Some(id) = device_id.filter(|id| !id.trim().is_empty()) {
        query.push(("device_id", id));
    }
    api_call(state, method, path, &query).await.map(|_| ())
}

#[tauri::command]
pub async fn spotify_play(
    device_id: Option<String>,
    state: tauri::State<'_, SpotifyState>,
) -> Result<(), String> {
    control(&state, Method::PUT, "/me/player/play", device_id, vec![]).await
}
#[tauri::command]
pub async fn spotify_pause(
    device_id: Option<String>,
    state: tauri::State<'_, SpotifyState>,
) -> Result<(), String> {
    control(&state, Method::PUT, "/me/player/pause", device_id, vec![]).await
}
#[tauri::command]
pub async fn spotify_next(
    device_id: Option<String>,
    state: tauri::State<'_, SpotifyState>,
) -> Result<(), String> {
    control(&state, Method::POST, "/me/player/next", device_id, vec![]).await
}
#[tauri::command]
pub async fn spotify_previous(
    device_id: Option<String>,
    state: tauri::State<'_, SpotifyState>,
) -> Result<(), String> {
    control(
        &state,
        Method::POST,
        "/me/player/previous",
        device_id,
        vec![],
    )
    .await
}
#[tauri::command]
pub async fn spotify_seek(
    position_ms: u64,
    device_id: Option<String>,
    state: tauri::State<'_, SpotifyState>,
) -> Result<(), String> {
    control(
        &state,
        Method::PUT,
        "/me/player/seek",
        device_id,
        vec![("position_ms", position_ms.to_string())],
    )
    .await
}
#[tauri::command]
pub async fn spotify_set_volume(
    volume_percent: u8,
    device_id: Option<String>,
    state: tauri::State<'_, SpotifyState>,
) -> Result<(), String> {
    control(
        &state,
        Method::PUT,
        "/me/player/volume",
        device_id,
        vec![("volume_percent", volume_percent.min(100).to_string())],
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_playback_is_explicit() {
        let playback = parse_playback(None);
        assert!(!playback.active);
        assert!(playback.track.is_none());
    }
    #[test]
    fn parses_device() {
        let value = serde_json::json!({"id":"x","name":"Desk","type":"Computer","is_active":true,"is_restricted":false,"volume_percent":120});
        assert_eq!(parse_device(&value).unwrap().volume_percent, Some(100));
    }
}
