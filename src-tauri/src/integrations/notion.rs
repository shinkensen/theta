use super::{mcp, oauth, storage, IntegrationStatus};
use serde::{Deserialize, Serialize};
use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
const RESOURCE: &str = "https://mcp.notion.com/mcp";
const RESOURCE_METADATA: &str = "https://mcp.notion.com/mcp/.well-known/oauth-protected-resource";
#[derive(Default, Clone, Serialize, Deserialize)]
struct Auth {
    client_id: Option<String>,
    client_secret: Option<String>,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    refresh_token: Option<String>,
    access_token: Option<String>,
    expires_at: Option<i64>,
    account_label: Option<String>,
}
pub struct NotionState {
    auth: Mutex<Auth>,
    session: Mutex<Option<mcp::McpSession>>,
    path: PathBuf,
    http: reqwest::Client,
}
impl NotionState {
    pub fn load(dir: &Path) -> Self {
        Self {
            auth: Mutex::new(storage::load_or_default(&dir.join("notion_auth.json"))),
            session: Mutex::new(None),
            path: dir.join("notion_auth.json"),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(40))
                .user_agent("Theta Notion MCP")
                .build()
                .unwrap_or_default(),
        }
    }
    fn snapshot(&self) -> Result<Auth, String> {
        self.auth
            .lock()
            .map(|v| v.clone())
            .map_err(|_| "Notion credentials are unavailable".into())
    }
    fn update(&self, f: impl FnOnce(&mut Auth)) -> Result<(), String> {
        let mut auth = self
            .auth
            .lock()
            .map_err(|_| "Notion credentials are unavailable")?;
        f(&mut auth);
        storage::save_json(&self.path, &*auth, "Notion")
    }
    async fn token(&self) -> Result<String, String> {
        let a = self.snapshot()?;
        let now = chrono::Utc::now().timestamp();
        if let (Some(token), Some(expiry)) = (&a.access_token, a.expires_at) {
            if expiry > now + 120 {
                return Ok(token.clone());
            }
        }
        let refresh = a.refresh_token.ok_or("Notion is not connected")?;
        let client = a.client_id.ok_or("Notion OAuth registration is missing")?;
        let endpoint = a.token_endpoint.ok_or("Notion token endpoint is missing")?;
        let mut fields = vec![
            ("grant_type", "refresh_token".into()),
            ("refresh_token", refresh),
            ("client_id", client),
        ];
        if let Some(secret) = a.client_secret {
            fields.push(("client_secret", secret))
        }
        let response = self
            .http
            .post(endpoint)
            .form(&fields)
            .send()
            .await
            .map_err(|e| format!("Notion token refresh failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "Notion rejected token refresh ({status}). {}",
                oauth::clip_error(&text)
            ));
        }
        let token: Token = serde_json::from_str(&text)
            .map_err(|e| format!("Invalid Notion token response: {e}"))?;
        let access = token.access_token.clone();
        self.update(|a| {
            a.access_token = Some(token.access_token);
            a.expires_at = Some(now + token.expires_in.unwrap_or(28800));
            if token.refresh_token.is_some() {
                a.refresh_token = token.refresh_token
            }
        })?;
        Ok(access)
    }
    fn session(&self) -> Result<mcp::McpSession, String> {
        self.session
            .lock()
            .map_err(|_| "Notion session is unavailable")?
            .clone()
            .ok_or("Notion MCP is not connected".into())
    }
}
#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    workspace_id: Option<String>,
    email_domain: Option<String>,
}
#[tauri::command]
pub fn notion_status(state: tauri::State<'_, NotionState>) -> Result<IntegrationStatus, String> {
    let a = state.snapshot()?;
    let connected = state
        .session
        .lock()
        .map_err(|_| "Notion session is unavailable")?
        .is_some();
    Ok(IntegrationStatus {
        id: "notion",
        name: "Notion",
        configured: a.client_id.is_some(),
        connected,
        account_label: a.account_label,
        redirect_uri: Some(oauth::LOOPBACK_REDIRECT),
        message: Some("Connects to Notion's official hosted MCP using OAuth and PKCE.".into()),
    })
}
async fn discover(
    state: &NotionState,
    redirect: &str,
) -> Result<(String, String, String, Option<String>), String> {
    let protected: serde_json::Value = state
        .http
        .get(RESOURCE_METADATA)
        .send()
        .await
        .map_err(|e| format!("Notion OAuth discovery failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Invalid Notion resource metadata: {e}"))?;
    let server = protected
        .get("authorization_servers")
        .and_then(|v| v.as_array())
        .and_then(|v| v.first())
        .and_then(|v| v.as_str())
        .ok_or("Notion did not advertise an authorization server")?;
    let metadata: serde_json::Value = state
        .http
        .get(format!("{server}/.well-known/oauth-authorization-server"))
        .send()
        .await
        .map_err(|e| format!("Notion authorization discovery failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Invalid Notion authorization metadata: {e}"))?;
    let authorization = metadata
        .get("authorization_endpoint")
        .and_then(|v| v.as_str())
        .ok_or("Notion authorization endpoint missing")?
        .to_string();
    let token = metadata
        .get("token_endpoint")
        .and_then(|v| v.as_str())
        .ok_or("Notion token endpoint missing")?
        .to_string();
    let registration = metadata
        .get("registration_endpoint")
        .and_then(|v| v.as_str())
        .ok_or("Notion dynamic registration endpoint missing")?;
    let registered:serde_json::Value=state.http.post(registration).json(&serde_json::json!({"client_name":"Theta Desktop Assistant","redirect_uris":[redirect],"grant_types":["authorization_code","refresh_token"],"response_types":["code"],"token_endpoint_auth_method":"none"})).send().await.map_err(|e|format!("Notion dynamic registration failed: {e}"))?.json().await.map_err(|e|format!("Invalid Notion registration response: {e}"))?;
    let client = registered
        .get("client_id")
        .and_then(|v| v.as_str())
        .ok_or("Notion registration did not return a client ID")?
        .to_string();
    let secret = registered
        .get("client_secret")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    Ok((authorization, token, client, secret))
}
#[tauri::command]
pub async fn notion_connect(state: tauri::State<'_, NotionState>) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Couldn't open the Notion callback port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let (ae, te, client, secret) = discover(&state, &redirect).await?;
    let (verifier, challenge) = oauth::pkce_pair();
    let csrf = oauth::csrf_token();
    let url=format!("{ae}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256&prompt=consent",oauth::encode(&client),oauth::encode(&redirect),oauth::encode(&csrf),oauth::encode(&challenge));
    tauri_plugin_opener::open_url(&url, None::<&str>)
        .map_err(|e| format!("Couldn't open Notion sign-in: {e}"))?;
    let callback = tauri::async_runtime::spawn_blocking(move || {
        oauth::await_callback(&listener, 300, "Notion")
    })
    .await
    .map_err(|e| format!("Notion OAuth task failed: {e}"))??;
    if callback.state.as_deref() != Some(&csrf) {
        return Err("Notion OAuth state mismatch — sign-in aborted".into());
    }
    let code = callback
        .code
        .ok_or("Notion did not return an authorization code")?;
    let mut fields = vec![
        ("grant_type", "authorization_code".into()),
        ("code", code),
        ("client_id", client.clone()),
        ("redirect_uri", redirect),
        ("code_verifier", verifier),
    ];
    if let Some(value) = secret.clone() {
        fields.push(("client_secret", value))
    }
    let response = state
        .http
        .post(&te)
        .form(&fields)
        .send()
        .await
        .map_err(|e| format!("Notion token exchange failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Notion rejected sign-in ({status}). {}",
            oauth::clip_error(&text)
        ));
    }
    let token: Token =
        serde_json::from_str(&text).map_err(|e| format!("Invalid Notion token response: {e}"))?;
    let access = token.access_token.clone();
    let label = token
        .email_domain
        .clone()
        .or(token.workspace_id.clone())
        .unwrap_or_else(|| "Notion workspace".into());
    let expiry = chrono::Utc::now().timestamp() + token.expires_in.unwrap_or(28800);
    state.update(|a| {
        a.client_id = Some(client);
        a.client_secret = secret;
        a.authorization_endpoint = Some(ae);
        a.token_endpoint = Some(te);
        a.access_token = Some(access.clone());
        a.refresh_token = token.refresh_token;
        a.expires_at = Some(expiry);
        a.account_label = Some(label.clone())
    })?;
    let mut session = mcp::initialize(&state.http, RESOURCE, Some(&access)).await?;
    session.tools = mcp::list_tools(&state.http, RESOURCE, Some(&access), &session).await?;
    *state
        .session
        .lock()
        .map_err(|_| "Notion session is unavailable")? = Some(session);
    Ok(label)
}
#[tauri::command]
pub async fn notion_disconnect(state: tauri::State<'_, NotionState>) -> Result<(), String> {
    let token = state.snapshot()?.access_token;
    let session = state
        .session
        .lock()
        .map_err(|_| "Notion session is unavailable")?
        .take();
    if let (Some(token), Some(session)) = (token.as_deref(), session.as_ref()) {
        mcp::close_session(&state.http, RESOURCE, Some(token), session).await
    }
    state.update(|a| {
        a.refresh_token = None;
        a.access_token = None;
        a.expires_at = None;
        a.account_label = None
    })
}
#[tauri::command]
pub fn notion_list_tools(
    state: tauri::State<'_, NotionState>,
) -> Result<Vec<mcp::McpTool>, String> {
    Ok(state.session()?.tools)
}
#[tauri::command]
pub async fn notion_call_tool(
    name: String,
    args: serde_json::Value,
    state: tauri::State<'_, NotionState>,
) -> Result<mcp::McpCallResult, String> {
    let token = state.token().await?;
    let session = state.session()?;
    mcp::call_tool(&state.http, RESOURCE, Some(&token), &session, &name, args).await
}
