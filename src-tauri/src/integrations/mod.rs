pub mod github;
pub mod gmail;
pub mod hackatime;
pub mod mcp;
pub mod minestrator;
pub mod notion;
pub mod oauth;
pub mod spotify;
pub mod storage;

use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub configured: bool,
    pub connected: bool,
    pub account_label: Option<String>,
    pub redirect_uri: Option<&'static str>,
    pub message: Option<String>,
}
