use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

pub const LOOPBACK_REDIRECT: &str = "http://127.0.0.1/callback";

pub struct CallbackResult {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce_pair() -> (String, String) {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    let verifier: String = (0..64)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect();
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub fn csrf_token() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    b64url(&bytes)
}

pub fn encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

pub fn clip_error(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= 400 {
        trimmed.to_string()
    } else {
        trimmed.chars().take(400).collect()
    }
}

pub fn await_callback(
    listener: &TcpListener,
    timeout_secs: u64,
    provider: &str,
) -> Result<CallbackResult, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Couldn't configure the loopback listener: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|e| format!("Loopback stream error: {e}"))?;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut request_line = String::new();
                BufReader::new(
                    stream
                        .try_clone()
                        .map_err(|e| format!("Loopback stream error: {e}"))?,
                )
                .read_line(&mut request_line)
                .map_err(|e| format!("Couldn't read the OAuth redirect: {e}"))?;
                let page = "<!doctype html><meta charset=utf-8><title>Theta</title><body style=\"font-family:system-ui;background:#0a0e14;color:#f5f5f7;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h2>Theta is connected.</h2><p>You can close this tab.</p></div>";
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        page.len(), page
                    )
                    .as_bytes(),
                );
                let _ = stream.flush();
                return Ok(parse_callback_request(&request_line));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(format!("Timed out waiting for {provider} sign-in."));
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(format!("Loopback listener failed: {e}")),
        }
    }
}

fn parse_callback_request(request_line: &str) -> CallbackResult {
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
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
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| value.to_string());
        match key {
            "code" => result.code = Some(decoded),
            "state" => result.state = Some(decoded),
            "error" => result.error = Some(decoded),
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_values_have_expected_shape() {
        let (verifier, challenge) = pkce_pair();
        assert_eq!(verifier.len(), 64);
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
    }

    #[test]
    fn parses_callback_query() {
        let result = parse_callback_request("GET /callback?code=a%20b&state=s HTTP/1.1");
        assert_eq!(result.code.as_deref(), Some("a b"));
        assert_eq!(result.state.as_deref(), Some("s"));
    }
}
