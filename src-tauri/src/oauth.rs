//! Google OAuth 2.0 for installed apps: loopback redirect + PKCE (RFC 8252).
//! The refresh token goes straight into the OS keychain and never reaches the UI.

use crate::error::{AppError, AppResult};
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/drive";
const TIMEOUT: Duration = Duration::from_secs(300);

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Pkce {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let verifier = b64(&raw);
    let challenge = b64(&Sha256::digest(verifier.as_bytes()));
    Pkce { verifier, challenge }
}

pub fn auth_url(client_id: &str, redirect: &str, challenge: &str, state: &str) -> String {
    let enc = |s: &str| urlencoding::encode(s).into_owned();
    format!(
        "{AUTH_URL}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        enc(client_id),
        enc(redirect),
        enc(SCOPE),
        enc(challenge),
        enc(state)
    )
}

/// Outcome of one request to the loopback listener.
#[derive(Debug, PartialEq)]
pub enum Redirect {
    /// Not our redirect (favicon, preconnect, wrong/missing state): ignore and keep waiting.
    Unrelated,
    Code(String),
    Denied(String),
}

/// Parses the redirect request line. `state` is validated first, so a stray local
/// request can neither complete nor abort the sign-in.
pub fn parse_redirect(request: &str, expected_state: &str) -> Redirect {
    let line = request.lines().next().unwrap_or("");
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let (mut code, mut state, mut error) = (None, None, None);
    for kv in query.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let v = urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_default();
        match k {
            "code" => code = Some(v),
            "state" => state = Some(v),
            "error" => error = Some(v),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Redirect::Unrelated;
    }
    match (code, error) {
        (_, Some(e)) => Redirect::Denied(e),
        (Some(c), None) if !c.is_empty() => Redirect::Code(c),
        _ => Redirect::Unrelated,
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>5files</title><body style=\"font-family:system-ui;display:grid;place-items:center;height:100vh;margin:0;background:#0b0d12;color:#e7e9ee\"><div style=\"text-align:center\"><h2>You're signed in ✓</h2><p style=\"opacity:.7\">You can close this tab and return to 5files.</p></div>";

/// Runs the whole flow. `open` launches the system browser on the consent URL.
pub async fn google_sign_in(client_id: &str, client_secret: &str, open: impl FnOnce(&str) -> AppResult<()>) -> AppResult<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let p = pkce();
    let mut st = [0u8; 16];
    rand::rng().fill_bytes(&mut st);
    let state = b64(&st);
    open(&auth_url(client_id, &redirect, &p.challenge, &state))?;

    // Each connection is served by its own task with a read timeout, so an idle browser
    // preconnect can't stall the real redirect arriving on another socket.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Redirect>(8);
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let (tx, state) = (tx.clone(), state.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut buf)).await else { return };
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let outcome = parse_redirect(&req, &state);
                let (status, body) = match &outcome {
                    Redirect::Unrelated => ("404 Not Found", String::new()),
                    Redirect::Code(_) => ("200 OK", DONE_PAGE.to_string()),
                    Redirect::Denied(e) => {
                        ("200 OK", format!("<!doctype html><meta charset=utf-8><p>Sign-in was cancelled: {}</p>", html_escape(e)))
                    }
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                if outcome != Redirect::Unrelated {
                    let _ = tx.send(outcome).await;
                }
            });
        }
    });
    let outcome = tokio::time::timeout(TIMEOUT, rx.recv()).await;
    accept.abort();
    let code = match outcome {
        Ok(Some(Redirect::Code(c))) => c,
        Ok(Some(Redirect::Denied(e))) => return Err(AppError::Auth(format!("Google sign-in was denied: {e}"))),
        _ => return Err(AppError::Auth("timed out waiting for Google sign-in".into())),
    };

    #[derive(serde::Deserialize)]
    struct TokenResp {
        refresh_token: Option<String>,
        error: Option<String>,
        error_description: Option<String>,
    }
    let resp: TokenResp = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", p.verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| AppError::Connection(e.to_string()))?
        .json()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    if let Some(e) = resp.error {
        return Err(AppError::Auth(format!("{e}: {}", resp.error_description.unwrap_or_default())));
    }
    resp.refresh_token.ok_or_else(|| AppError::Auth("Google did not return a refresh token".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let p = pkce();
        assert_eq!(p.challenge, b64(&Sha256::digest(p.verifier.as_bytes())));
        assert!(p.verifier.len() >= 43);
    }

    #[test]
    fn redirect_parsing_validates_state_first() {
        let req = "GET /?state=abc&code=4%2F0Ad&scope=x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(parse_redirect(req, "abc"), Redirect::Code("4/0Ad".into()));
        assert_eq!(parse_redirect(req, "zzz"), Redirect::Unrelated);
        // A stray request with an error but the wrong state must not abort the flow.
        assert_eq!(parse_redirect("GET /?error=x&state=nope HTTP/1.1", "abc"), Redirect::Unrelated);
        assert_eq!(parse_redirect("GET /?error=access_denied&state=abc HTTP/1.1", "abc"), Redirect::Denied("access_denied".into()));
        assert_eq!(parse_redirect("GET /favicon.ico HTTP/1.1", "abc"), Redirect::Unrelated);
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
    }

    #[test]
    fn auth_url_contains_pkce_and_offline_access() {
        let u = auth_url("id.apps", "http://127.0.0.1:5000", "chal", "st");
        assert!(u.contains("code_challenge=chal"));
        assert!(u.contains("access_type=offline"));
        assert!(u.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5000"));
    }
}
