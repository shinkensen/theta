use super::{mcp, storage, IntegrationStatus};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
const BASE_ENDPOINT: &str = "https://mcp.sttr.io/minestrator";
#[derive(Default, Clone, Serialize, Deserialize)]
struct Auth {
    api_key: Option<String>,
    read_only: bool,
    toolsets: Vec<String>,
}
pub struct MineStratorState {
    auth: Mutex<Auth>,
    session: Mutex<Option<mcp::McpSession>>,
    path: PathBuf,
    http: reqwest::Client,
    last_error: Mutex<Option<String>>,
}
impl MineStratorState {
    pub fn load(dir: &Path) -> Self {
        let mut auth: Auth = storage::load_or_default(&dir.join("minestrator_auth.json"));
        if auth.toolsets.is_empty() {
            auth.read_only = true;
            auth.toolsets = vec!["core".into(), "files".into()]
        }
        Self {
            auth: Mutex::new(auth),
            session: Mutex::new(None),
            path: dir.join("minestrator_auth.json"),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(40))
                .user_agent("Theta MineStrator MCP")
                .build()
                .unwrap_or_default(),
            last_error: Mutex::new(None),
        }
    }
    fn auth(&self) -> Result<Auth, String> {
        self.auth
            .lock()
            .map(|v| v.clone())
            .map_err(|_| "MineStrator credentials are unavailable".into())
    }
    fn endpoint(auth: &Auth) -> String {
        format!(
            "{BASE_ENDPOINT}?readonly={}&toolsets={}",
            if auth.read_only { "1" } else { "0" },
            urlencoding::encode(&auth.toolsets.join(","))
        )
    }
    fn save(&self, auth: &Auth) -> Result<(), String> {
        storage::save_json(&self.path, auth, "MineStrator")
    }
    fn session(&self) -> Result<mcp::McpSession, String> {
        self.session
            .lock()
            .map_err(|_| "MineStrator session is unavailable")?
            .clone()
            .ok_or("MineStrator is not connected".into())
    }
}
#[tauri::command]
pub fn minestrator_status(
    state: tauri::State<'_, MineStratorState>,
) -> Result<IntegrationStatus, String> {
    let auth = state.auth()?;
    let session = state
        .session
        .lock()
        .map_err(|_| "MineStrator session is unavailable")?;
    let count = session.as_ref().map(|s| s.tools.len()).unwrap_or(0);
    let error = state.last_error.lock().ok().and_then(|v| v.clone());
    Ok(IntegrationStatus {
        id: "minestrator",
        name: "MineStrator",
        configured: auth.api_key.is_some(),
        connected: session.is_some(),
        account_label: Some(format!("{} discovered tools", count)),
        redirect_uri: None,
        message: error.or_else(|| {
            Some(format!(
                "{} mode · toolsets: {}",
                if auth.read_only { "Read-only" } else { "Full" },
                auth.toolsets.join(", ")
            ))
        }),
    })
}
#[tauri::command]
pub fn minestrator_save_config(
    api_key: String,
    read_only: Option<bool>,
    toolsets: Option<Vec<String>>,
    state: tauri::State<'_, MineStratorState>,
) -> Result<(), String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("Enter a MineStrator API key".into());
    }
    let allowed = ["core", "files", "backups", "databases", "servers"];
    let selected = toolsets.unwrap_or_else(|| vec!["core".into(), "files".into()]);
    if selected.is_empty() || selected.iter().any(|v| !allowed.contains(&v.as_str())) {
        return Err("MineStrator toolsets contain an unsupported value".into());
    }
    let auth = Auth {
        api_key: Some(key.into()),
        read_only: read_only.unwrap_or(true),
        toolsets: selected,
    };
    state.save(&auth)?;
    *state
        .auth
        .lock()
        .map_err(|_| "MineStrator credentials are unavailable")? = auth;
    Ok(())
}
#[tauri::command]
pub async fn minestrator_connect(
    state: tauri::State<'_, MineStratorState>,
) -> Result<usize, String> {
    let auth = state.auth()?;
    let key = auth
        .api_key
        .as_deref()
        .ok_or("Save a MineStrator API key first")?;
    let endpoint = MineStratorState::endpoint(&auth);
    let mut session = mcp::initialize(&state.http, &endpoint, Some(key)).await?;
    session.tools = mcp::list_tools(&state.http, &endpoint, Some(key), &session).await?;
    let count = session.tools.len();
    *state
        .session
        .lock()
        .map_err(|_| "MineStrator session is unavailable")? = Some(session);
    if let Ok(mut e) = state.last_error.lock() {
        *e = None
    }
    Ok(count)
}
#[tauri::command]
pub async fn minestrator_disconnect(
    state: tauri::State<'_, MineStratorState>,
) -> Result<(), String> {
    let auth = state.auth()?;
    let session = state
        .session
        .lock()
        .map_err(|_| "MineStrator session is unavailable")?
        .take();
    if let (Some(key), Some(session)) = (auth.api_key.as_deref(), session.as_ref()) {
        mcp::close_session(
            &state.http,
            &MineStratorState::endpoint(&auth),
            Some(key),
            session,
        )
        .await
    }
    let mut cleared = auth;
    cleared.api_key = None;
    state.save(&cleared)?;
    *state
        .auth
        .lock()
        .map_err(|_| "MineStrator credentials are unavailable")? = cleared;
    Ok(())
}
#[tauri::command]
pub fn minestrator_list_tools(
    state: tauri::State<'_, MineStratorState>,
) -> Result<Vec<mcp::McpTool>, String> {
    Ok(state.session()?.tools)
}
#[tauri::command]
pub async fn minestrator_call_tool(
    name: String,
    args: serde_json::Value,
    state: tauri::State<'_, MineStratorState>,
) -> Result<mcp::McpCallResult, String> {
    let auth = state.auth()?;
    let key = auth
        .api_key
        .as_deref()
        .ok_or("MineStrator is not configured")?;
    let session = state.session()?;
    mcp::call_tool(
        &state.http,
        &MineStratorState::endpoint(&auth),
        Some(key),
        &session,
        &name,
        args,
    )
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_is_fixed_and_filtered() {
        let value = MineStratorState::endpoint(&Auth {
            api_key: None,
            read_only: true,
            toolsets: vec!["core".into(), "files".into()],
        });
        assert_eq!(
            value,
            "https://mcp.sttr.io/minestrator?readonly=1&toolsets=core%2Cfiles"
        );
    }
}
