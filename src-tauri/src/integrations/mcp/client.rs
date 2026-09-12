use super::types::{McpCallResult, McpSession, McpTool};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};

const PROTOCOL: &str = "2025-06-18";
const MAX_RESULT_CHARS: usize = 14_000;

async fn request(
    client: &reqwest::Client,
    endpoint: &str,
    bearer: Option<&str>,
    session: Option<&McpSession>,
    id: Option<u64>,
    method: &str,
    params: Option<Value>,
) -> Result<(Option<String>, Value), String> {
    let body = if let Some(id) = id {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params.unwrap_or_else(||json!({}))})
    } else {
        json!({"jsonrpc":"2.0","method":method,"params":params.unwrap_or_else(||json!({}))})
    };
    let mut req = client
        .post(endpoint)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header(
            "MCP-Protocol-Version",
            session
                .map(|s| s.protocol_version.as_str())
                .unwrap_or(PROTOCOL),
        )
        .json(&body);
    if let Some(token) = bearer {
        req = req.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(value) = session.and_then(|s| s.id.as_deref()) {
        req = req.header("Mcp-Session-Id", value);
    }
    let response = req
        .send()
        .await
        .map_err(|e| format!("MCP request failed: {e}"))?;
    let status = response.status();
    let new_session = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "MCP server returned {status}: {}",
            crate::integrations::oauth::clip_error(&text)
        ));
    }
    if id.is_none() {
        return Ok((new_session, json!({})));
    }
    let value = if content_type.contains("text/event-stream") {
        parse_sse(&text)?
    } else {
        serde_json::from_str(&text).map_err(|e| format!("Invalid MCP JSON response: {e}"))?
    };
    if let Some(error) = value.get("error") {
        return Err(format!(
            "MCP error: {}",
            crate::integrations::oauth::clip_error(&error.to_string())
        ));
    }
    Ok((new_session, value.get("result").cloned().unwrap_or(value)))
}
fn parse_sse(text: &str) -> Result<Value, String> {
    for block in text.split("\n\n").collect::<Vec<_>>().into_iter().rev() {
        let data = block
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .collect::<String>();
        if !data.is_empty() {
            if let Ok(value) = serde_json::from_str(&data) {
                return Ok(value);
            }
        }
    }
    Err("MCP SSE response did not contain JSON data".into())
}
pub async fn initialize(
    client: &reqwest::Client,
    endpoint: &str,
    bearer: Option<&str>,
) -> Result<McpSession, String> {
    let(session_id,result)=request(client,endpoint,bearer,None,Some(1),"initialize",Some(json!({"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"Theta","version":env!("CARGO_PKG_VERSION")}}))).await?;
    let version = result
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .ok_or("MCP server did not negotiate a protocol version")?;
    if version != PROTOCOL {
        return Err(format!("Unsupported MCP protocol version: {version}"));
    }
    let session = McpSession {
        id: session_id,
        protocol_version: version.into(),
        tools: vec![],
    };
    request(
        client,
        endpoint,
        bearer,
        Some(&session),
        None,
        "notifications/initialized",
        None,
    )
    .await?;
    Ok(session)
}

pub async fn list_tools(
    client: &reqwest::Client,
    endpoint: &str,
    bearer: Option<&str>,
    session: &McpSession,
) -> Result<Vec<McpTool>, String> {
    let mut cursor: Option<String> = None;
    let mut tools = Vec::new();
    for page in 0..20 {
        let params = cursor.as_ref().map(|value| json!({"cursor":value}));
        let (_, result) = request(
            client,
            endpoint,
            bearer,
            Some(session),
            Some(10 + page),
            "tools/list",
            params,
        )
        .await?;
        let rows: Vec<McpTool> =
            serde_json::from_value(result.get("tools").cloned().unwrap_or_else(|| json!([])))
                .map_err(|e| format!("Invalid MCP tool list: {e}"))?;
        for tool in rows {
            if tools
                .iter()
                .any(|existing: &McpTool| existing.name == tool.name)
            {
                return Err(format!("MCP server returned duplicate tool: {}", tool.name));
            }
            tools.push(tool)
        }
        cursor = result
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if cursor.is_none() {
            return Ok(tools);
        }
    }
    Err("MCP tool list exceeded the pagination limit".into())
}
fn validate_args(schema: &Value, args: &Value) -> Result<(), String> {
    if !args.is_object() {
        return Err("MCP tool arguments must be an object".into());
    }
    if schema.get("type").and_then(|v| v.as_str()) == Some("object") {
        if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
            for key in required.iter().filter_map(|v| v.as_str()) {
                if args.get(key).is_none() {
                    return Err(format!("Missing required MCP argument: {key}"));
                }
            }
        }
        if schema.get("additionalProperties").and_then(|v| v.as_bool()) == Some(false) {
            if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                for key in args.as_object().into_iter().flat_map(|v| v.keys()) {
                    if !properties.contains_key(key) {
                        return Err(format!("Unknown MCP argument: {key}"));
                    }
                }
            }
        }
    }
    Ok(())
}
pub async fn call_tool(
    client: &reqwest::Client,
    endpoint: &str,
    bearer: Option<&str>,
    session: &McpSession,
    name: &str,
    args: Value,
) -> Result<McpCallResult, String> {
    let tool = session
        .tools
        .iter()
        .find(|tool| tool.name == name)
        .ok_or("MCP tool is not in the current tool list")?;
    validate_args(&tool.input_schema, &args)?;
    let (_, result) = request(
        client,
        endpoint,
        bearer,
        Some(session),
        Some(100),
        "tools/call",
        Some(json!({"name":name,"arguments":args})),
    )
    .await?;
    let mut content = result
        .get("structuredContent")
        .cloned()
        .or_else(|| result.get("content").cloned())
        .unwrap_or(Value::Null);
    let encoded = content.to_string();
    if encoded.chars().count() > MAX_RESULT_CHARS {
        content = Value::String(format!(
            "{}\n[truncated]",
            encoded.chars().take(MAX_RESULT_CHARS).collect::<String>()
        ))
    }
    Ok(McpCallResult {
        content,
        is_error: result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}
pub async fn close_session(
    client: &reqwest::Client,
    endpoint: &str,
    bearer: Option<&str>,
    session: &McpSession,
) {
    let Some(id) = session.id.as_deref() else {
        return;
    };
    let mut req = client
        .delete(endpoint)
        .header("Mcp-Session-Id", id)
        .header("MCP-Protocol-Version", &session.protocol_version);
    if let Some(token) = bearer {
        req = req.bearer_auth(token)
    }
    let _ = req.send().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_sse_data() {
        let value =
            parse_sse("event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n")
                .unwrap();
        assert_eq!(value["id"], 1)
    }
    #[test]
    fn validates_required_arguments() {
        let schema = json!({"type":"object","required":["id"]});
        assert!(validate_args(&schema, &json!({})).is_err());
    }
}
