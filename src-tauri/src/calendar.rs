//! Google Calendar, via an installed-app OAuth 2.0 flow with PKCE.
//!
//! The flow is the loopback variant: we bind an ephemeral port on
//! 127.0.0.1, send the user to Google's consent screen with that port as the
//! redirect URI, and read the authorization code out of the single request
//! Google's redirect makes back to us. No embedded webview, no client secret
//! in the frontend bundle, and nothing to configure in the code — the client
//! id/secret are entered once in Settings and persisted next to the tokens.
//!
//! Tokens live in `<app-data>/google_auth.json`. Access tokens are refreshed
//! automatically when they are within [`REFRESH_SKEW_SECS`] of expiring.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const API_BASE: &str = "https://www.googleapis.com/calendar/v3";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar";

/// Refresh this many seconds before actual expiry.
const REFRESH_SKEW_SECS: i64 = 120;

/// How long to wait for the user to finish the consent screen.
const CONSENT_TIMEOUT_SECS: u64 = 300;

#[derive(Serialize, Deserialize, Default, Clone)]
struct AuthFile {
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    access_token: Option<String>,
    /// Unix seconds.
    expires_at: Option<i64>,
    email: Option<String>,
}

pub struct CalendarState {
    auth: Mutex<AuthFile>,
    path: PathBuf,
    http: reqwest::Client,
}

impl CalendarState {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("google_auth.json");
        let auth = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AuthFile>(&raw).ok())
            .unwrap_or_default();
        Self {
            auth: Mutex::new(auth),
            path,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(25))
                .build()
                .unwrap_or_default(),
        }
    }

    fn persist(&self, auth: &AuthFile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(auth).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| format!("Failed to save Google auth: {e}"))
    }

    fn snapshot(&self) -> Result<AuthFile, String> {
        self.auth
            .lock()
            .map(|a| a.clone())
            .map_err(|e| e.to_string())
    }

    fn update(&self, f: impl FnOnce(&mut AuthFile)) -> Result<AuthFile, String> {
        let mut guard = self.auth.lock().map_err(|e| e.to_string())?;
        f(&mut guard);
        let snapshot = guard.clone();
        drop(guard);
        self.persist(&snapshot)?;
        Ok(snapshot)
    }

    /// Returns a valid bearer token, refreshing first if it's expiring.
    async fn access_token(&self) -> Result<String, String> {
        let auth = self.snapshot()?;
        let now = chrono::Utc::now().timestamp();

        if let (Some(token), Some(exp)) = (&auth.access_token, auth.expires_at) {
            if exp - REFRESH_SKEW_SECS > now {
                return Ok(token.clone());
            }
        }

        let (Some(client_id), Some(refresh)) = (&auth.client_id, &auth.refresh_token) else {
            return Err(
                "Google Calendar isn't connected. Open Settings → Google Calendar and connect."
                    .into(),
            );
        };

        let mut form = vec![
            ("client_id", client_id.clone()),
            ("refresh_token", refresh.clone()),
            ("grant_type", "refresh_token".to_string()),
        ];
        if let Some(secret) = auth.client_secret.as_deref().filter(|s| !s.is_empty()) {
            form.push(("client_secret", secret.to_string()));
        }

        let res = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("Token refresh request failed: {e}"))?;

        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "Google rejected the token refresh ({status}). Reconnect in Settings. {}",
                clip_error(&body)
            ));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: Option<i64>,
        }
        let parsed: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Couldn't parse the refresh response: {e}"))?;

        let expires_at = chrono::Utc::now().timestamp() + parsed.expires_in.unwrap_or(3600);
        let token = parsed.access_token.clone();
        self.update(|a| {
            a.access_token = Some(parsed.access_token);
            a.expires_at = Some(expires_at);
        })?;
        Ok(token)
    }
}

/// Google error bodies can be long; keep enough to be actionable.
fn clip_error(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= 400 {
        return trimmed.to_string();
    }
    trimmed.chars().take(400).collect()
}

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE S256 pair: a random verifier and its SHA-256 challenge.
fn pkce_pair() -> (String, String) {
    use rand::Rng;
    use sha2::{Digest, Sha256};

    let verifier: String = {
        let mut rng = rand::rng();
        (0..64)
            .map(|_| {
                const CHARSET: &[u8] =
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
                CHARSET[rng.random_range(0..CHARSET.len())] as char
            })
            .collect()
    };
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

#[derive(Serialize)]
pub struct AuthStatus {
    pub connected: bool,
    pub has_credentials: bool,
    pub email: Option<String>,
}

#[tauri::command]
pub fn google_auth_status(state: tauri::State<'_, CalendarState>) -> Result<AuthStatus, String> {
    let auth = state.snapshot()?;
    Ok(AuthStatus {
        connected: auth.refresh_token.is_some(),
        has_credentials: auth.client_id.is_some(),
        email: auth.email,
    })
}

/// Stores the OAuth client id/secret from Settings. Changing the client id
/// invalidates any existing tokens, so those are cleared.
#[tauri::command]
pub fn google_set_credentials(
    client_id: String,
    client_secret: Option<String>,
    state: tauri::State<'_, CalendarState>,
) -> Result<(), String> {
    let client_id = client_id.trim().to_string();
    if client_id.is_empty() {
        return Err("Client ID can't be empty.".into());
    }
    let changed = state
        .snapshot()?
        .client_id
        .map(|existing| existing != client_id)
        .unwrap_or(true);

    state.update(|a| {
        a.client_id = Some(client_id);
        a.client_secret = client_secret.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if changed {
            a.refresh_token = None;
            a.access_token = None;
            a.expires_at = None;
            a.email = None;
        }
    })?;
    Ok(())
}

#[tauri::command]
pub fn google_disconnect(state: tauri::State<'_, CalendarState>) -> Result<(), String> {
    state.update(|a| {
        a.refresh_token = None;
        a.access_token = None;
        a.expires_at = None;
        a.email = None;
    })?;
    Ok(())
}

/// The single HTTP request Google's redirect makes back to us.
struct CallbackResult {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

const CONSENT_DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Theta</title>\
<body style=\"font-family:system-ui;background:#0a0e14;color:#f5f5f7;display:grid;\
place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\">\
<div style=\"font-size:44px;color:#f59e0b\">&theta;</div>\
<h2 style=\"font-weight:600\">Theta is connected.</h2>\
<p style=\"color:#a0a4ab\">You can close this tab.</p></div>";

/// Waits for the redirect, replies with a small page, returns the query params.
fn await_callback(listener: &TcpListener, timeout_secs: u64) -> Result<CallbackResult, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Couldn't configure the loopback listener: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|e| format!("Loopback stream error: {e}"))?;
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));

                let mut request_line = String::new();
                BufReader::new(
                    stream
                        .try_clone()
                        .map_err(|e| format!("Loopback stream error: {e}"))?,
                )
                .read_line(&mut request_line)
                .map_err(|e| format!("Couldn't read the OAuth redirect: {e}"))?;

                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        CONSENT_DONE_PAGE.len(),
                        CONSENT_DONE_PAGE
                    )
                    .as_bytes(),
                );
                let _ = stream.flush();

                // `GET /?code=...&state=... HTTP/1.1`
                let target = request_line.split_whitespace().nth(1).unwrap_or("");
                let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
                let mut result = CallbackResult {
                    code: None,
                    state: None,
                    error: None,
                };
                for pair in query.split('&') {
                    let Some((key, value)) = pair.split_once('=') else {
                        continue;
                    };
                    let decoded = urlencoding::decode(value)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| value.to_string());
                    match key {
                        "code" => result.code = Some(decoded),
                        "state" => result.state = Some(decoded),
                        "error" => result.error = Some(decoded),
                        _ => {}
                    }
                }
                return Ok(result);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err("Timed out waiting for Google sign-in.".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            Err(e) => return Err(format!("Loopback listener failed: {e}")),
        }
    }
}

/// Runs the full consent flow and stores the resulting refresh token.
///
/// Opens the system browser, waits for the redirect on a loopback port, then
/// exchanges the code. Returns the connected account's email when Google
/// includes one in the id token.
#[tauri::command]
pub async fn google_connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, CalendarState>,
) -> Result<String, String> {
    let auth = state.snapshot()?;
    let client_id = auth.client_id.clone().ok_or(
        "No OAuth client ID saved yet. Paste one in Settings → Google Calendar first.",
    )?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Couldn't open a loopback port for the OAuth redirect: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let (verifier, challenge) = pkce_pair();
    let csrf = b64url(&{
        use rand::Rng;
        let mut bytes = [0u8; 16];
        rand::rng().fill(&mut bytes);
        bytes
    });

    let enc = |s: &str| urlencoding::encode(s).into_owned();
    let auth_url = format!(
        "{AUTH_ENDPOINT}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &code_challenge={}&code_challenge_method=S256&state={}\
         &access_type=offline&prompt=consent",
        enc(&client_id),
        enc(&redirect_uri),
        enc(SCOPE),
        enc(&challenge),
        enc(&csrf),
    );

    tauri_plugin_opener::open_url(&auth_url, None::<&str>)
        .map_err(|e| format!("Couldn't open the browser for Google sign-in: {e}"))?;
    let _ = app;

    let expected_csrf = csrf.clone();
    let callback =
        tauri::async_runtime::spawn_blocking(move || await_callback(&listener, CONSENT_TIMEOUT_SECS))
            .await
            .map_err(|e| format!("OAuth wait task failed: {e}"))??;

    if let Some(err) = callback.error {
        return Err(format!("Google sign-in was denied: {err}"));
    }
    if callback.state.as_deref() != Some(expected_csrf.as_str()) {
        return Err("OAuth state mismatch — sign-in aborted for safety.".into());
    }
    let code = callback
        .code
        .ok_or("Google's redirect didn't include an authorization code.")?;

    let mut form = vec![
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code".to_string()),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(secret) = auth.client_secret.as_deref().filter(|s| !s.is_empty()) {
        form.push(("client_secret", secret.to_string()));
    }

    let res = state
        .http
        .post(TOKEN_ENDPOINT)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("Token exchange request failed: {e}"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Google rejected the token exchange ({status}). {}",
            clip_error(&body)
        ));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
        id_token: Option<String>,
    }
    let parsed: TokenResponse = serde_json::from_str(&body)
        .map_err(|e| format!("Couldn't parse Google's token response: {e}"))?;

    let refresh_token = parsed.refresh_token.ok_or(
        "Google didn't return a refresh token. Remove Theta at \
         myaccount.google.com/permissions and connect again.",
    )?;
    let email = parsed.id_token.as_deref().and_then(email_from_id_token);
    let expires_at = chrono::Utc::now().timestamp() + parsed.expires_in.unwrap_or(3600);

    state.update(|a| {
        a.refresh_token = Some(refresh_token);
        a.access_token = Some(parsed.access_token);
        a.expires_at = Some(expires_at);
        a.email = email.clone();
    })?;

    Ok(email.unwrap_or_else(|| "connected".to_string()))
}

/// Reads the `email` claim out of an id token's payload.
///
/// The token comes straight from Google's TLS-authenticated token endpoint,
/// so this is a display convenience, not a security decision — no signature
/// verification is needed or performed.
fn email_from_id_token(id_token: &str) -> Option<String> {
    use base64::Engine;
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("email")?
        .as_str()
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Calendar v3
// ---------------------------------------------------------------------------

/// A flattened event, shaped for both the UI table and the LLM's tool result.
#[derive(Serialize, Clone)]
pub struct CalEvent {
    pub id: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    /// RFC 3339 for timed events, `YYYY-MM-DD` for all-day ones.
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub calendar_id: String,
    pub html_link: Option<String>,
    pub status: Option<String>,
    pub attendees: Vec<String>,
}

#[derive(Serialize)]
pub struct CalendarSummary {
    pub id: String,
    pub summary: String,
    pub primary: bool,
    pub access_role: Option<String>,
}

/// One place where every Calendar API call runs, so auth, status handling and
/// error text stay consistent.
async fn api_call(
    state: &CalendarState,
    method: reqwest::Method,
    url: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let token = state.access_token().await?;
    let mut req = state.http.request(method.clone(), &url).bearer_auth(token);
    if let Some(payload) = body {
        req = req.json(&payload);
    } else if method == reqwest::Method::POST {
        // Some Google frontends reject bodyless POSTs unless the length is explicit.
        req = req.header(reqwest::header::CONTENT_LENGTH, 0);
    }

    let res = req
        .send()
        .await
        .map_err(|e| format!("Calendar request failed: {e}"))?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();

    if !status.is_success() {
        // Pull Google's own message out when present — it's far more useful
        // than the bare status.
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| clip_error(&text));
        return Err(match status.as_u16() {
            401 => format!("Google says the session expired ({detail}). Reconnect in Settings."),
            403 => format!("Google denied the request: {detail}"),
            404 => format!("Not found: {detail}"),
            _ => format!("Calendar API error {status}: {detail}"),
        });
    }

    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| format!("Couldn't parse the Calendar response: {e}"))
}

fn parse_event(raw: &serde_json::Value, calendar_id: &str) -> Option<CalEvent> {
    let id = raw.get("id")?.as_str()?.to_string();
    // All-day events carry `date`; timed ones carry `dateTime`.
    let pick = |key: &str| -> (String, bool) {
        let node = raw.get(key);
        let date_time = node
            .and_then(|n| n.get("dateTime"))
            .and_then(|v| v.as_str());
        match date_time {
            Some(dt) => (dt.to_string(), false),
            None => (
                node.and_then(|n| n.get("date"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                true,
            ),
        }
    };
    let (start, start_all_day) = pick("start");
    let (end, _) = pick("end");

    Some(CalEvent {
        id,
        summary: raw
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("(untitled)")
            .to_string(),
        description: raw
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        location: raw
            .get("location")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        start,
        end,
        all_day: start_all_day,
        calendar_id: calendar_id.to_string(),
        html_link: raw
            .get("htmlLink")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        status: raw
            .get("status")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        attendees: raw
            .get("attendees")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|a| a.get("email").and_then(|e| e.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[tauri::command]
pub async fn calendar_list_calendars(
    state: tauri::State<'_, CalendarState>,
) -> Result<Vec<CalendarSummary>, String> {
    let json = api_call(
        &state,
        reqwest::Method::GET,
        format!("{API_BASE}/users/me/calendarList?minAccessRole=reader"),
        None,
    )
    .await?;

    Ok(json
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|c| {
                    Some(CalendarSummary {
                        id: c.get("id")?.as_str()?.to_string(),
                        summary: c
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(unnamed)")
                            .to_string(),
                        primary: c
                            .get("primary")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        access_role: c
                            .get("accessRole")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Lists events in a window.
///
/// `time_min`/`time_max` accept either RFC 3339 or a bare `YYYY-MM-DD`, which
/// is expanded to that day in the machine's local timezone — the LLM reliably
/// produces plain dates, and interpreting them as UTC would shift events
/// across day boundaries for anyone not on UTC.
#[tauri::command]
pub async fn calendar_list_events(
    time_min: Option<String>,
    time_max: Option<String>,
    calendar_id: Option<String>,
    query: Option<String>,
    max_results: Option<u32>,
    state: tauri::State<'_, CalendarState>,
) -> Result<Vec<CalEvent>, String> {
    let calendar_id = calendar_id.unwrap_or_else(|| "primary".to_string());
    let enc = |s: &str| urlencoding::encode(s).into_owned();

    let min = to_rfc3339(time_min.as_deref(), false)?;
    let max = to_rfc3339(time_max.as_deref(), true)?;
    let mut url = format!(
        "{API_BASE}/calendars/{}/events?singleEvents=true&orderBy=startTime\
         &timeMin={}&timeMax={}&maxResults={}",
        enc(&calendar_id),
        enc(&min),
        enc(&max),
        max_results.unwrap_or(50).clamp(1, 250),
    );
    if let Some(q) = query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        url.push_str(&format!("&q={}", enc(q)));
    }

    let json = api_call(&state, reqwest::Method::GET, url, None).await?;
    Ok(json
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|e| parse_event(e, &calendar_id))
                .collect()
        })
        .unwrap_or_default())
}

/// Normalizes a caller-supplied instant to RFC 3339 with an offset.
///
/// * `None`             → now (or now + 7 days when `end_of_day`)
/// * `YYYY-MM-DD`       → local midnight, or 23:59:59 local when `end_of_day`
/// * anything else      → passed through, assumed already RFC 3339
fn to_rfc3339(value: Option<&str>, end_of_day: bool) -> Result<String, String> {
    use chrono::{Local, NaiveDate, TimeZone};

    let Some(raw) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        let now = chrono::Utc::now();
        let instant = if end_of_day {
            now + chrono::Duration::days(7)
        } else {
            now
        };
        return Ok(instant.to_rfc3339());
    };

    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let naive = if end_of_day {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        }
        .ok_or("Invalid time of day.")?;
        return Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|dt| dt.to_rfc3339())
            .ok_or_else(|| format!("'{raw}' is ambiguous in the local timezone."));
    }

    Ok(raw.to_string())
}

/// What the UI and the LLM send when creating or editing an event.
///
/// Everything except `summary` is optional so the same struct can drive a
/// `PATCH`, where omitted fields must stay untouched on Google's side.
#[derive(Deserialize, Default)]
pub struct EventInput {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    /// RFC 3339, `YYYY-MM-DDTHH:MM(:SS)` (assumed local), or `YYYY-MM-DD`.
    pub start: Option<String>,
    pub end: Option<String>,
    #[serde(default)]
    pub all_day: bool,
    pub attendees: Option<Vec<String>>,
}

/// Builds a Calendar v3 `start`/`end` node from loose user/LLM input.
///
/// Timed events are sent as `dateTime` carrying an explicit UTC offset and no
/// `timeZone`, which is the one combination that can't be misinterpreted —
/// naming an IANA zone would mean shipping a whole tz database.
fn event_time(raw: &str, all_day: bool) -> Result<serde_json::Value, String> {
    use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};

    let raw = raw.trim();
    if all_day {
        // Accept a full timestamp here too and just keep the date part.
        let date = NaiveDate::parse_from_str(&raw[..raw.len().min(10)], "%Y-%m-%d")
            .map_err(|_| format!("'{raw}' isn't a date I can read (want YYYY-MM-DD)."))?;
        return Ok(serde_json::json!({ "date": date.to_string() }));
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(serde_json::json!({ "dateTime": dt.to_rfc3339() }));
    }

    // No offset supplied → interpret in the machine's local zone.
    const LOCAL_FORMATS: [&str; 4] = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ];
    let naive = LOCAL_FORMATS
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(raw, f).ok())
        .or_else(|| {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
        })
        .ok_or_else(|| format!("'{raw}' isn't a time I can read (want YYYY-MM-DDTHH:MM)."))?;

    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| serde_json::json!({ "dateTime": dt.to_rfc3339() }))
        .ok_or_else(|| format!("'{raw}' doesn't exist in the local timezone (DST gap)."))
}

/// `end` when the caller didn't give one: +1 hour for timed events, +1 day for
/// all-day ones (Calendar treats an all-day `end` as exclusive).
fn default_end(start: &serde_json::Value, all_day: bool) -> Result<serde_json::Value, String> {
    use chrono::{DateTime, Duration, NaiveDate};

    if all_day {
        let date = start
            .get("date")
            .and_then(|v| v.as_str())
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .ok_or("Couldn't work out the day after the start date.")?;
        let next = date
            .succ_opt()
            .ok_or("Start date is out of the supported range.")?;
        return Ok(serde_json::json!({ "date": next.to_string() }));
    }

    let dt = start
        .get("dateTime")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .ok_or("Couldn't work out an end time from the start time.")?;
    Ok(serde_json::json!({ "dateTime": (dt + Duration::hours(1)).to_rfc3339() }))
}

/// Translates [`EventInput`] into a Calendar v3 event body.
///
/// `require_start` is true for creates (Google rejects an event with no start)
/// and false for patches, where absent fields mean "leave as-is".
fn event_body(input: &EventInput, require_start: bool) -> Result<serde_json::Value, String> {
    let mut body = serde_json::Map::new();

    if let Some(s) = input.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        body.insert("summary".into(), s.into());
    } else if require_start {
        body.insert("summary".into(), "(untitled)".into());
    }
    for (key, value) in [
        ("description", input.description.as_deref()),
        ("location", input.location.as_deref()),
    ] {
        if let Some(v) = value {
            body.insert(key.into(), v.trim().into());
        }
    }
    if let Some(list) = input.attendees.as_ref() {
        body.insert(
            "attendees".into(),
            serde_json::Value::Array(
                list.iter()
                    .map(|e| e.trim())
                    .filter(|e| !e.is_empty())
                    .map(|e| serde_json::json!({ "email": e }))
                    .collect(),
            ),
        );
    }

    match input.start.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let start = event_time(raw, input.all_day)?;
            let end = match input.end.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(e) => event_time(e, input.all_day)?,
                None => default_end(&start, input.all_day)?,
            };
            body.insert("start".into(), start);
            body.insert("end".into(), end);
        }
        None if require_start => {
            return Err("An event needs a start time.".into());
        }
        None => {
            // A patch that only moves the end time is legitimate.
            if let Some(e) = input.end.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                body.insert("end".into(), event_time(e, input.all_day)?);
            }
        }
    }

    Ok(serde_json::Value::Object(body))
}

/// Creates an event. Gated behind a confirmation card in the UI.
#[tauri::command]
pub async fn calendar_create_event(
    event: EventInput,
    calendar_id: Option<String>,
    state: tauri::State<'_, CalendarState>,
) -> Result<CalEvent, String> {
    let calendar_id = calendar_id.unwrap_or_else(|| "primary".to_string());
    let body = event_body(&event, true)?;
    let json = api_call(
        &state,
        reqwest::Method::POST,
        format!(
            "{API_BASE}/calendars/{}/events",
            urlencoding::encode(&calendar_id)
        ),
        Some(body),
    )
    .await?;
    parse_event(&json, &calendar_id).ok_or("Google accepted the event but returned no id.".into())
}

/// Partial update — `PATCH`, so fields left out of `event` stay as they are.
#[tauri::command]
pub async fn calendar_update_event(
    event_id: String,
    event: EventInput,
    calendar_id: Option<String>,
    state: tauri::State<'_, CalendarState>,
) -> Result<CalEvent, String> {
    if event_id.trim().is_empty() {
        return Err("No event id to update.".into());
    }
    let calendar_id = calendar_id.unwrap_or_else(|| "primary".to_string());
    let body = event_body(&event, false)?;
    if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return Err("Nothing to change on that event.".into());
    }
    let json = api_call(
        &state,
        reqwest::Method::PATCH,
        format!(
            "{API_BASE}/calendars/{}/events/{}",
            urlencoding::encode(&calendar_id),
            urlencoding::encode(event_id.trim())
        ),
        Some(body),
    )
    .await?;
    parse_event(&json, &calendar_id).ok_or("Google accepted the edit but returned no event.".into())
}

/// Deletes an event. Returns a sentence the assistant can read back.
#[tauri::command]
pub async fn calendar_delete_event(
    event_id: String,
    calendar_id: Option<String>,
    state: tauri::State<'_, CalendarState>,
) -> Result<String, String> {
    if event_id.trim().is_empty() {
        return Err("No event id to delete.".into());
    }
    let calendar_id = calendar_id.unwrap_or_else(|| "primary".to_string());
    api_call(
        &state,
        reqwest::Method::DELETE,
        format!(
            "{API_BASE}/calendars/{}/events/{}",
            urlencoding::encode(&calendar_id),
            urlencoding::encode(event_id.trim())
        ),
        None,
    )
    .await?;
    Ok("Event deleted.".into())
}

/// Natural-language create ("lunch with Sam tomorrow at noon").
///
/// Google does the parsing, which is more reliable than asking the model to
/// emit RFC 3339 — but it can't set attendees or descriptions, so the
/// structured path above still exists.
#[tauri::command]
pub async fn calendar_quick_add(
    text: String,
    calendar_id: Option<String>,
    state: tauri::State<'_, CalendarState>,
) -> Result<CalEvent, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Nothing to add.".into());
    }
    let calendar_id = calendar_id.unwrap_or_else(|| "primary".to_string());
    let json = api_call(
        &state,
        reqwest::Method::POST,
        format!(
            "{API_BASE}/calendars/{}/events/quickAdd?text={}",
            urlencoding::encode(&calendar_id),
            urlencoding::encode(&text)
        ),
        None,
    )
    .await?;
    parse_event(&json, &calendar_id).ok_or("Google accepted the event but returned no id.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_day_input_becomes_a_date_node() {
        let node = event_time("2026-03-04", true).unwrap();
        assert_eq!(node["date"], "2026-03-04");
        // A full timestamp is tolerated for all-day events.
        let node = event_time("2026-03-04T09:00:00Z", true).unwrap();
        assert_eq!(node["date"], "2026-03-04");
    }

    #[test]
    fn offsetless_times_get_a_local_offset() {
        let node = event_time("2026-03-04T14:30", false).unwrap();
        let text = node["dateTime"].as_str().unwrap();
        assert!(text.starts_with("2026-03-04T14:30:00"));
        // Must be a valid RFC 3339 instant, i.e. it carries an offset or Z.
        assert!(chrono::DateTime::parse_from_rfc3339(text).is_ok());
    }

    #[test]
    fn missing_end_is_filled_in() {
        let input = EventInput {
            summary: Some("standup".into()),
            start: Some("2026-03-04T09:00:00Z".into()),
            ..Default::default()
        };
        let body = event_body(&input, true).unwrap();
        let end = body["end"]["dateTime"].as_str().unwrap();
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(end).unwrap().to_utc(),
            chrono::DateTime::parse_from_rfc3339("2026-03-04T10:00:00Z")
                .unwrap()
                .to_utc()
        );

        // All-day events get an exclusive next-day end instead.
        let input = EventInput {
            start: Some("2026-03-04".into()),
            all_day: true,
            ..Default::default()
        };
        let body = event_body(&input, true).unwrap();
        assert_eq!(body["end"]["date"], "2026-03-05");
    }

    #[test]
    fn a_patch_may_omit_the_start() {
        let input = EventInput {
            location: Some("Room 2".into()),
            ..Default::default()
        };
        assert!(event_body(&input, false).unwrap()["location"] == "Room 2");
        // …but a create can't.
        assert!(event_body(&input, true).is_err());
    }

    #[test]
    fn bare_dates_expand_to_local_day_bounds() {
        let start = to_rfc3339(Some("2026-03-04"), false).unwrap();
        let end = to_rfc3339(Some("2026-03-04"), true).unwrap();
        let (start, end) = (
            chrono::DateTime::parse_from_rfc3339(&start).unwrap(),
            chrono::DateTime::parse_from_rfc3339(&end).unwrap(),
        );
        assert!(end > start);
        // Same calendar day, just under 24h apart.
        assert!((end - start).num_hours() == 23);
    }

    #[test]
    fn event_parsing_flattens_both_time_shapes() {
        let timed = serde_json::json!({
            "id": "abc",
            "summary": "1:1",
            "start": { "dateTime": "2026-03-04T09:00:00Z" },
            "end": { "dateTime": "2026-03-04T09:30:00Z" },
            "attendees": [{ "email": "sam@example.com" }, { "displayName": "no email" }]
        });
        let ev = parse_event(&timed, "primary").unwrap();
        assert!(!ev.all_day);
        assert_eq!(ev.start, "2026-03-04T09:00:00Z");
        assert_eq!(ev.attendees, vec!["sam@example.com"]);

        let all_day = serde_json::json!({
            "id": "xyz",
            "start": { "date": "2026-03-04" },
            "end": { "date": "2026-03-05" }
        });
        let ev = parse_event(&all_day, "work").unwrap();
        assert!(ev.all_day);
        assert_eq!(ev.summary, "(untitled)");
        assert_eq!(ev.calendar_id, "work");
    }
}
