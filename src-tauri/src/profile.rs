use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

const VERSION: u8 = 1;
const MAX_PER_CATEGORY: usize = 12;
const SENSITIVE: &[&str] = &[
    "password",
    "api key",
    "secret",
    "credit card",
    "medical",
    "diagnosis",
    "religion",
    "politic",
    "sexual",
    "precise address",
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Interest,
    Hobby,
    Project,
    Preference,
    RecurringTopic,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProfileItem {
    pub id: String,
    pub category: Category,
    pub text: String,
    pub confidence: f32,
    pub evidence_count: u32,
    pub explicit: bool,
    pub created: i64,
    pub last_seen: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOperation {
    pub category: Category,
    pub text: String,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub replace_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Persisted {
    version: u8,
    items: Vec<ProfileItem>,
}

impl Default for Persisted {
    fn default() -> Self {
        Self {
            version: VERSION,
            items: Vec::new(),
        }
    }
}

pub struct ProfileState {
    inner: Mutex<Persisted>,
    path: PathBuf,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn key(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn valid(text: &str) -> bool {
    let normal = key(text);
    !normal.is_empty() && normal.len() <= 180 && !SENSITIVE.iter().any(|term| normal.contains(term))
}

impl ProfileState {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("profile.json");
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self {
            inner: Mutex::new(inner),
            path,
        }
    }

    fn save(&self, profile: &Persisted) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(profile).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| format!("Couldn't save About me: {e}"))
    }

    fn apply(&self, operations: Vec<ProfileOperation>) -> Result<Vec<ProfileItem>, String> {
        let mut profile = self.inner.lock().map_err(|e| e.to_string())?;
        let timestamp = now();
        profile.items.retain(|item| {
            item.explicit || item.confidence >= 0.35 || timestamp - item.last_seen < 90 * 86_400
        });
        for op in operations.into_iter().take(8) {
            let text = op.text.trim();
            if !valid(text) {
                continue;
            }
            if let Some(id) = op.replace_id.as_deref() {
                profile.items.retain(|item| item.id != id);
            }
            let normal = key(text);
            if let Some(item) = profile
                .items
                .iter_mut()
                .find(|item| item.category == op.category && key(&item.text) == normal)
            {
                item.evidence_count += 1;
                item.last_seen = timestamp;
                item.explicit |= op.explicit;
                item.confidence = (item.confidence + if op.explicit { 0.3 } else { 0.14 }).min(1.0);
            } else {
                profile.items.push(ProfileItem {
                    id: format!("{:x}-{}", timestamp, rand::random::<u32>()),
                    category: op.category,
                    text: text.to_string(),
                    confidence: if op.explicit { 0.85 } else { 0.35 },
                    evidence_count: 1,
                    explicit: op.explicit,
                    created: timestamp,
                    last_seen: timestamp,
                });
            }
        }
        for category in [
            Category::Interest,
            Category::Hobby,
            Category::Project,
            Category::Preference,
            Category::RecurringTopic,
        ] {
            let mut matching: Vec<_> = profile
                .items
                .iter()
                .filter(|item| item.category == category)
                .map(|item| (item.id.clone(), item.confidence, item.last_seen))
                .collect();
            matching.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
            for (id, _, _) in matching.into_iter().skip(MAX_PER_CATEGORY) {
                profile.items.retain(|item| item.id != id);
            }
        }
        self.save(&profile)?;
        Ok(profile.items.clone())
    }
}

#[tauri::command]
pub fn profile_get(state: tauri::State<'_, ProfileState>) -> Result<Vec<ProfileItem>, String> {
    let profile = state.inner.lock().map_err(|e| e.to_string())?;
    Ok(profile.items.clone())
}

#[tauri::command]
pub fn profile_apply(
    operations: Vec<ProfileOperation>,
    state: tauri::State<'_, ProfileState>,
) -> Result<Vec<ProfileItem>, String> {
    state.apply(operations)
}

#[tauri::command]
pub fn profile_remove(id: String, state: tauri::State<'_, ProfileState>) -> Result<usize, String> {
    let mut profile = state.inner.lock().map_err(|e| e.to_string())?;
    let before = profile.items.len();
    profile.items.retain(|item| item.id != id);
    state.save(&profile)?;
    Ok(before - profile.items.len())
}

#[tauri::command]
pub fn profile_clear(state: tauri::State<'_, ProfileState>) -> Result<(), String> {
    let mut profile = state.inner.lock().map_err(|e| e.to_string())?;
    profile.items.clear();
    state.save(&profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation_rejects_sensitive_data() {
        assert!(!valid("My API key is abc"));
        assert!(valid("Enjoys landscape photography"));
    }
    #[test]
    fn normalisation_is_stable() {
        assert_eq!(key("Rust & Tauri!"), "rust tauri");
    }
}
