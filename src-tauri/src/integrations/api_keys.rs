use super::{storage, IntegrationStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[derive(Default, Clone, Serialize, Deserialize)]
struct ApiKeys {
    openrouter_key: Option<String>,
    firecrawl_key: Option<String>,
}
pub struct ApiKeyState {
    keys: Mutex<ApiKeys>,
    path: PathBuf,
}
impl ApiKeyState {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("api_keys.json");
        let keys = storage::load_or_default(&path);
        Self {
            keys: Mutex::new(keys),
            path,
        }
    }
    fn snapshot(&self) -> Result<ApiKeys, String> {
        self.keys
            .lock()
            .map(|k| k.clone())
            .map_err(|_| "API keys are unavailable".into())
    }
    fn update(&self, f: impl FnOnce(&mut ApiKeys)) -> Result<ApiKeys, String> {
        let mut guard = self.keys.lock().map_err(|_| "API keys are unavailable")?;
        f(&mut guard);
        let snapshot = guard.clone();
        storage::save_json(&self.path, &snapshot, "API keys")?;
        Ok(snapshot)
    }
}
#[tauri::command]
pub fn openrouter_status(
    state: tauri::State<'_, ApiKeyState>,
) -> Result<IntegrationStatus, String> {
    let keys = state.snapshot()?;
    Ok(IntegrationStatus {
        id: "openrouter",
        name: "OpenRouter",
        configured: keys.openrouter_key.is_some(),
        connected: keys.openrouter_key.is_some(),
        account_label: None,
        redirect_uri: None,
        message: None,
    })
}
#[tauri::command]
pub fn openrouter_save_key(
    api_key: String,
    state: tauri::State<'_, ApiKeyState>,
) -> Result<(), String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("OpenRouter API key cannot be empty".into());
    }
    state.update(|keys| {
        keys.openrouter_key = Some(trimmed.into());
    })?;
    Ok(())
}
#[tauri::command]
pub fn openrouter_get_key(state: tauri::State<'_, ApiKeyState>) -> Result<String, String> {
    let keys = state.snapshot()?;
    keys.openrouter_key
        .ok_or_else(|| "OpenRouter API key is not configured".into())
}
#[tauri::command]
pub fn openrouter_clear_key(state: tauri::State<'_, ApiKeyState>) -> Result<(), String> {
    state.update(|keys| {
        keys.openrouter_key = None;
    })?;
    Ok(())
}
#[tauri::command]
pub fn firecrawl_status(state: tauri::State<'_, ApiKeyState>) -> Result<IntegrationStatus, String> {
    let keys = state.snapshot()?;
    Ok(IntegrationStatus {
        id: "firecrawl",
        name: "Firecrawl",
        configured: keys.firecrawl_key.is_some(),
        connected: keys.firecrawl_key.is_some(),
        account_label: None,
        redirect_uri: None,
        message: None,
    })
}
#[tauri::command]
pub fn firecrawl_save_key(
    api_key: String,
    state: tauri::State<'_, ApiKeyState>,
) -> Result<(), String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("Firecrawl API key cannot be empty".into());
    }
    state.update(|keys| {
        keys.firecrawl_key = Some(trimmed.into());
    })?;
    Ok(())
}
#[tauri::command]
pub fn firecrawl_get_key(state: tauri::State<'_, ApiKeyState>) -> Result<String, String> {
    let keys = state.snapshot()?;
    keys.firecrawl_key
        .ok_or_else(|| "Firecrawl API key is not configured".into())
}
