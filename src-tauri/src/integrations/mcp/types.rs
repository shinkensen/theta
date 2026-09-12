use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default)]
pub struct McpSession {
    pub id: Option<String>,
    pub protocol_version: String,
    pub tools: Vec<McpTool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallResult {
    pub content: serde_json::Value,
    pub is_error: bool,
}
