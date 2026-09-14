
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub timestamp: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created: u64,
    pub updated: u64,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created: u64,
    pub updated: u64,
    pub message_count: usize,
}

pub struct ConversationState {
    inner: Mutex<Option<String>>, 
    conversations_dir: PathBuf,
}

impl ConversationState {
    pub fn new(app_data_dir: &Path) -> Result<Self, String> {
        let conversations_dir = app_data_dir.join("conversations");
        fs::create_dir_all(&conversations_dir)
            .map_err(|e| format!("Failed to create conversations directory: {e}"))?;

        Ok(Self {
            inner: Mutex::new(None),
            conversations_dir,
        })
    }

    fn conversation_path(&self, id: &str) -> PathBuf {
        self.conversations_dir.join(format!("{}.json", id))
    }

    pub fn load(&self, id: &str) -> Result<Conversation, String> {
        let path = self.conversation_path(id);
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read conversation {}: {}", id, e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse conversation {}: {}", id, e))
    }

    pub fn save(&self, conversation: &Conversation) -> Result<(), String> {
        let path = self.conversation_path(&conversation.id);
        let content = serde_json::to_string_pretty(conversation)
            .map_err(|e| format!("Failed to serialize conversation: {e}"))?;
        fs::write(&path, content)
            .map_err(|e| format!("Failed to write conversation: {e}"))?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<ConversationSummary>, String> {
        let entries = fs::read_dir(&self.conversations_dir)
            .map_err(|e| format!("Failed to read conversations directory: {e}"))?;

        let mut summaries = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(conv) = serde_json::from_str::<Conversation>(&content) {
                        summaries.push(ConversationSummary {
                            id: conv.id,
                            title: conv.title,
                            created: conv.created,
                            updated: conv.updated,
                            message_count: conv.messages.len(),
                        });
                    }
                }
            }
        }

        summaries.sort_by(|a, b| b.updated.cmp(&a.updated));
        Ok(summaries)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let path = self.conversation_path(id);
        fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete conversation {}: {}", id, e))?;
        Ok(())
    }

    pub fn set_current(&self, id: Option<String>) -> Result<(), String> {
        let mut current = self.inner.lock().map_err(|e| e.to_string())?;
        *current = id;
        Ok(())
    }

    pub fn get_current(&self) -> Result<Option<String>, String> {
        let current = self.inner.lock().map_err(|e| e.to_string())?;
        Ok(current.clone())
    }
}

#[tauri::command]
pub fn conversation_create(
    title: String,
    state: tauri::State<'_, ConversationState>,
) -> Result<Conversation, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as u64;

    let id = uuid::Uuid::new_v4().to_string();
    let conversation = Conversation {
        id: id.clone(),
        title,
        created: now,
        updated: now,
        messages: Vec::new(),
    };

    state.save(&conversation)?;
    state.set_current(Some(id))?;
    Ok(conversation)
}

#[tauri::command]
pub fn conversation_save(
    conversation: Conversation,
    state: tauri::State<'_, ConversationState>,
) -> Result<(), String> {
    state.save(&conversation)
}

#[tauri::command]
pub fn conversation_load(
    id: String,
    state: tauri::State<'_, ConversationState>,
) -> Result<Conversation, String> {
    state.load(&id)
}

#[tauri::command]
pub fn conversation_list(
    state: tauri::State<'_, ConversationState>,
) -> Result<Vec<ConversationSummary>, String> {
    state.list()
}

#[tauri::command]
pub fn conversation_delete(
    id: String,
    state: tauri::State<'_, ConversationState>,
) -> Result<(), String> {
    state.delete(&id)
}

#[tauri::command]
pub fn conversation_set_current(
    id: Option<String>,
    state: tauri::State<'_, ConversationState>,
) -> Result<(), String> {
    state.set_current(id)
}

#[tauri::command]
pub fn conversation_get_current(
    state: tauri::State<'_, ConversationState>,
) -> Result<Option<String>, String> {
    state.get_current()
}
