use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCOPE: &str = "https://www.googleapis.com/auth/youtube.upload";
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Clone, Deserialize)]
struct ClientSecretFile {
    installed: ClientSecret,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientSecret {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCache {
    pub refresh_token: String,
    pub access_token: String,
    pub expires_at: u64,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .expect("could not determine a config directory for this platform")
        .join("yt-upload")
}

pub fn client_secret_path() -> PathBuf {
    config_dir().join("client_secret.json")
}

pub fn token_cache_path() -> PathBuf {
    config_dir().join("token.json")
}

pub fn load_client_secret(path: &Path) -> anyhow::Result<ClientSecret> {
    let data = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("could not read client secret at {}: {e}", path.display()))?;
    let file: ClientSecretFile = serde_json::from_str(&data)?;
    Ok(file.installed)
}

pub fn load_token_cache(path: &Path) -> Option<TokenCache> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_token_cache(path: &Path, cache: &TokenCache) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        restrict_permissions(parent, 0o700)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(cache)?)?;
    restrict_permissions(path, 0o600)?;
    Ok(())
}

/// Restricts a file or directory's Unix permission bits. No-op on
/// non-Unix platforms, since this crate only documents macOS/Linux
/// support.
#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

pub fn build_authorize_url(client_id: &str, redirect_uri: &str) -> String {
    let mut url = url::Url::parse(AUTH_URL).expect("AUTH_URL is a valid URL");
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    url.to_string()
}

/// Extracts the `code` query parameter from the first line of a raw HTTP
/// request (as received on the OAuth loopback redirect listener).
fn extract_code_from_request_line(request: &str) -> Option<String> {
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs()
}

pub async fn exchange_code_for_token(
    client: &ClientSecret,
    code: &str,
    redirect_uri: &str,
) -> anyhow::Result<TokenCache> {
    let http = reqwest::Client::new();
    let resp: TokenResponse = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client.client_id.as_str()),
            ("client_secret", client.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(TokenCache {
        refresh_token: resp
            .refresh_token
            .ok_or_else(|| anyhow::anyhow!("no refresh_token in token response"))?,
        access_token: resp.access_token,
        expires_at: now_unix() + resp.expires_in,
    })
}

pub async fn refresh_access_token(
    client: &ClientSecret,
    cache: &TokenCache,
) -> anyhow::Result<TokenCache> {
    let http = reqwest::Client::new();
    let resp: TokenResponse = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client.client_id.as_str()),
            ("client_secret", client.client_secret.as_str()),
            ("refresh_token", cache.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(TokenCache {
        refresh_token: cache.refresh_token.clone(),
        access_token: resp.access_token,
        expires_at: now_unix() + resp.expires_in,
    })
}

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub async fn run_installed_app_flow(client: &ClientSecret) -> anyhow::Result<TokenCache> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let auth_url = build_authorize_url(&client.client_id, &redirect_uri);
    eprintln!("Open this URL in your browser to authorize yt-upload:\n{auth_url}");
    let _ = webbrowser::open(&auth_url);

    let (mut stream, _) = listener.accept().await?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let code = match extract_code_from_request_line(&request) {
        Some(code) => code,
        None => {
            let body =
                "Authorization was not granted. You can close this tab and return to the terminal.";
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            return Err(anyhow::anyhow!(
                "no authorization code found in OAuth redirect"
            ));
        }
    };

    let body = "Authorization received. You can close this tab and return to the terminal.";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;

    exchange_code_for_token(client, &code, &redirect_uri).await
}

/// Returns a valid access token, refreshing or running the full browser
/// consent flow as needed, and persisting the result to `cache_path`.
pub async fn get_valid_access_token(
    client: &ClientSecret,
    cache_path: &Path,
) -> anyhow::Result<String> {
    if let Some(cache) = load_token_cache(cache_path) {
        if cache.expires_at > now_unix() + 60 {
            return Ok(cache.access_token);
        }
        match refresh_access_token(client, &cache).await {
            Ok(refreshed) => {
                save_token_cache(cache_path, &refreshed)?;
                return Ok(refreshed.access_token);
            }
            Err(_) => {
                // The refresh token may have been revoked or expired.
                // Fall back to a fresh browser consent flow rather than
                // failing the whole program outright.
                let fresh = run_installed_app_flow(client).await?;
                save_token_cache(cache_path, &fresh)?;
                return Ok(fresh.access_token);
            }
        }
    }

    let fresh = run_installed_app_flow(client).await?;
    save_token_cache(cache_path, &fresh)?;
    Ok(fresh.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_authorize_url_contains_client_id_scope_and_redirect() {
        let url = build_authorize_url("abc123", "http://127.0.0.1:9000");
        assert!(url.contains("client_id=abc123"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A9000"));
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube.upload"));
        assert!(url.contains("response_type=code"));
    }

    #[test]
    fn load_client_secret_parses_installed_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client_secret.json");
        std::fs::write(
            &path,
            r#"{"installed":{"client_id":"id-1","client_secret":"secret-1"}}"#,
        )
        .unwrap();

        let secret = load_client_secret(&path).unwrap();
        assert_eq!(secret.client_id, "id-1");
        assert_eq!(secret.client_secret, "secret-1");
    }

    #[test]
    fn load_client_secret_errors_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        assert!(load_client_secret(&path).is_err());
    }

    #[test]
    fn token_cache_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token.json");
        let cache = TokenCache {
            refresh_token: "r-1".to_string(),
            access_token: "a-1".to_string(),
            expires_at: 1_700_000_000,
        };

        save_token_cache(&path, &cache).unwrap();
        let loaded = load_token_cache(&path).unwrap();
        assert_eq!(loaded.refresh_token, cache.refresh_token);
        assert_eq!(loaded.access_token, cache.access_token);
        assert_eq!(loaded.expires_at, cache.expires_at);
    }

    #[test]
    fn extract_code_from_request_line_parses_code() {
        let request = "GET /?code=4/xyz&scope=foo HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(
            extract_code_from_request_line(request),
            Some("4/xyz".to_string())
        );
    }

    #[test]
    fn extract_code_from_request_line_returns_none_without_code() {
        let request = "GET /favicon.ico HTTP/1.1\r\n\r\n";
        assert_eq!(extract_code_from_request_line(request), None);
    }
}
