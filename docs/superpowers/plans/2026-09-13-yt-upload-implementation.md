# yt-upload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a small Rust CLI that uploads a video to YouTube via the resumable upload protocol, with OAuth2 auth and resume-after-interruption.

**Architecture:** A `clap`-based CLI parses flags into metadata, an `auth` module handles OAuth2 (installed-app/loopback flow, cached refresh token), a `resumable_upload` module drives the chunked PUT protocol against the YouTube Data API v3, consulting a `state` module to resume an interrupted upload from the last confirmed byte instead of restarting.

**Tech Stack:** Rust, `clap`, `reqwest`, `tokio`, `serde`/`serde_json`, `dirs`, `sha2`, `url`, `webbrowser`, `anyhow`. Dev-dependency: `tempfile`.

Reference spec: `docs/superpowers/specs/2026-09-13-yt-upload-design.md`

---

### Task 1: Project scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `.gitignore`

- [ ] **Step 1: Create Cargo.toml with all dependencies**

```toml
[package]
name = "yt-upload"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", features = ["json"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dirs = "5"
sha2 = "0.10"
hex = "0.4"
url = "2"
webbrowser = "0.8"
anyhow = "1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create a minimal main.rs**

```rust
fn main() {
    println!("yt-upload");
}
```

- [ ] **Step 3: Create .gitignore**

```
/target
```

- [ ] **Step 4: Verify the project builds**

Run: `cargo build`
Expected: compiles successfully (dependency download may take a while on first run), producing `target/debug/yt-upload`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs .gitignore
git commit -m "Scaffold yt-upload crate with dependencies"
```

---

### Task 2: Chunk byte-range math (`chunk.rs`)

**Files:**
- Create: `src/chunk.rs`
- Modify: `src/main.rs` (add `mod chunk;`)

- [ ] **Step 1: Write the failing tests**

Create `src/chunk.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_chunk_starts_at_zero() {
        let range = next_chunk_range(0, 20_000_000, CHUNK_SIZE).unwrap();
        assert_eq!(range.start, 0);
        assert_eq!(range.end, CHUNK_SIZE - 1);
        assert_eq!(range.total, 20_000_000);
    }

    #[test]
    fn last_chunk_is_clamped_to_file_size() {
        let total = CHUNK_SIZE + 100;
        let range = next_chunk_range(CHUNK_SIZE, total, CHUNK_SIZE).unwrap();
        assert_eq!(range.start, CHUNK_SIZE);
        assert_eq!(range.end, total - 1);
        assert_eq!(range.len(), 100);
    }

    #[test]
    fn returns_none_when_fully_uploaded() {
        let total = 1000;
        assert!(next_chunk_range(total, total, CHUNK_SIZE).is_none());
    }

    #[test]
    fn content_range_header_format() {
        let range = ChunkRange { start: 0, end: 99, total: 500 };
        assert_eq!(range.content_range_header(), "bytes 0-99/500");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib chunk`
Expected: FAIL — `next_chunk_range`, `ChunkRange`, and `CHUNK_SIZE` are not defined yet.

- [ ] **Step 3: Implement chunk.rs above the test module**

Add this above the `#[cfg(test)] mod tests` block in `src/chunk.rs`:

```rust
/// Must be a multiple of 256 KiB per the YouTube resumable upload protocol.
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct ChunkRange {
    pub start: u64,
    pub end: u64, // inclusive
    pub total: u64,
}

impl ChunkRange {
    pub fn content_range_header(&self) -> String {
        format!("bytes {}-{}/{}", self.start, self.end, self.total)
    }

    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// Returns the next chunk to upload given how many bytes have been
/// confirmed uploaded so far, or `None` if the upload is complete.
pub fn next_chunk_range(uploaded: u64, total: u64, chunk_size: u64) -> Option<ChunkRange> {
    if uploaded >= total {
        return None;
    }
    let end = std::cmp::min(uploaded + chunk_size - 1, total - 1);
    Some(ChunkRange { start: uploaded, end, total })
}
```

- [ ] **Step 4: Register the module and run tests**

In `src/main.rs`, add above `fn main()`:

```rust
mod chunk;
```

Run: `cargo test --lib chunk`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/chunk.rs src/main.rs
git commit -m "Add chunk byte-range math for resumable upload"
```

---

### Task 3: Resume state persistence (`state.rs`)

**Files:**
- Create: `src/state.rs`
- Modify: `src/main.rs` (add `mod state;`)

- [ ] **Step 1: Write the failing tests**

Create `src/state.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn state_file_path_is_deterministic_for_same_path() {
        let a = state_file_path_for_key("/home/user/video.mp4");
        let b = state_file_path_for_key("/home/user/video.mp4");
        assert_eq!(a, b);
    }

    #[test]
    fn state_file_path_differs_for_different_paths() {
        let a = state_file_path_for_key("/home/user/video.mp4");
        let b = state_file_path_for_key("/home/user/other.mp4");
        assert_ne!(a, b);
    }

    #[test]
    fn save_and_load_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = UploadState {
            file_path: "/tmp/video.mp4".to_string(),
            file_size: 12345,
            file_mtime: 1_700_000_000,
            session_uri: "https://example.com/session".to_string(),
        };

        save_state(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn load_state_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        assert!(load_state(&path).is_none());
    }

    #[test]
    fn delete_state_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = UploadState {
            file_path: "/tmp/video.mp4".to_string(),
            file_size: 1,
            file_mtime: 1,
            session_uri: "https://example.com/session".to_string(),
        };
        save_state(&path, &state).unwrap();
        delete_state(&path);
        assert!(!path.exists());
    }

    #[test]
    fn matches_current_file_true_when_size_and_mtime_match() {
        let state = UploadState {
            file_path: "x".to_string(),
            file_size: 100,
            file_mtime: 200,
            session_uri: "y".to_string(),
        };
        assert!(matches_current_file(&state, 100, 200));
    }

    #[test]
    fn matches_current_file_false_when_size_differs() {
        let state = UploadState {
            file_path: "x".to_string(),
            file_size: 100,
            file_mtime: 200,
            session_uri: "y".to_string(),
        };
        assert!(!matches_current_file(&state, 999, 200));
    }

    #[test]
    fn matches_current_file_false_when_mtime_differs() {
        let state = UploadState {
            file_path: "x".to_string(),
            file_size: 100,
            file_mtime: 200,
            session_uri: "y".to_string(),
        };
        assert!(!matches_current_file(&state, 100, 999));
    }

    fn state_file_path_for_key(key: &str) -> PathBuf {
        hash_key_to_path(key)
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib state`
Expected: FAIL — `UploadState`, `save_state`, `load_state`, `delete_state`, `matches_current_file`, `hash_key_to_path` are not defined yet.

- [ ] **Step 3: Implement state.rs above the test module**

Add this above the `#[cfg(test)] mod tests` block in `src/state.rs`:

```rust
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadState {
    pub file_path: String,
    pub file_size: u64,
    pub file_mtime: u64,
    pub session_uri: String,
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .expect("could not determine a state directory for this platform")
        .join("yt-upload")
}

/// Hashes an arbitrary string key (typically an absolute file path) into a
/// stable filename under the state directory.
fn hash_key_to_path(key: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let hash = hex::encode(hasher.finalize());
    state_dir().join(format!("{hash}.json"))
}

pub fn state_file_path(video_path: &Path) -> PathBuf {
    let abs = std::fs::canonicalize(video_path).unwrap_or_else(|_| video_path.to_path_buf());
    hash_key_to_path(&abs.to_string_lossy())
}

pub fn load_state(state_path: &Path) -> Option<UploadState> {
    let data = std::fs::read_to_string(state_path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_state(state_path: &Path, state: &UploadState) -> std::io::Result<()> {
    if let Some(parent) = state_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(state)?;
    std::fs::write(state_path, data)
}

pub fn delete_state(state_path: &Path) {
    let _ = std::fs::remove_file(state_path);
}

/// Whether a previously stored session can be resumed for the file as it
/// currently exists on disk (same size and mtime), or should be discarded.
pub fn matches_current_file(state: &UploadState, current_size: u64, current_mtime: u64) -> bool {
    state.file_size == current_size && state.file_mtime == current_mtime
}
```

- [ ] **Step 4: Register the module and run tests**

In `src/main.rs`, add below `mod chunk;`:

```rust
mod state;
```

Run: `cargo test --lib state`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add src/state.rs src/main.rs
git commit -m "Add resume-state persistence for interrupted uploads"
```

---

### Task 4: CLI argument parsing (`cli.rs`)

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs` (add `mod cli;`)

- [ ] **Step 1: Write the failing tests**

Create `src/cli.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_file_and_title() {
        let args = Args::try_parse_from(["yt-upload", "video.mp4", "--title", "My Video"]).unwrap();
        assert_eq!(args.file.to_str().unwrap(), "video.mp4");
        assert_eq!(args.title, "My Video");
    }

    #[test]
    fn defaults_privacy_private_and_category_22() {
        let args = Args::try_parse_from(["yt-upload", "video.mp4", "--title", "T"]).unwrap();
        assert_eq!(args.privacy, Privacy::Private);
        assert_eq!(args.category, 22);
        assert_eq!(args.description, "");
        assert!(args.tags.is_empty());
    }

    #[test]
    fn tags_split_on_comma() {
        let args = Args::try_parse_from([
            "yt-upload", "video.mp4", "--title", "T", "--tags", "a,b,c",
        ])
        .unwrap();
        assert_eq!(args.tags, vec!["a", "b", "c"]);
    }

    #[test]
    fn privacy_accepts_unlisted_and_public() {
        let args = Args::try_parse_from([
            "yt-upload", "video.mp4", "--title", "T", "--privacy", "unlisted",
        ])
        .unwrap();
        assert_eq!(args.privacy, Privacy::Unlisted);
        assert_eq!(args.privacy.as_api_str(), "unlisted");
    }

    #[test]
    fn missing_title_is_an_error() {
        let result = Args::try_parse_from(["yt-upload", "video.mp4"]);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib cli`
Expected: FAIL — `Args`, `Privacy` are not defined yet.

- [ ] **Step 3: Implement cli.rs above the test module**

Add this above the `#[cfg(test)] mod tests` block in `src/cli.rs`:

```rust
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Privacy {
    Private,
    Unlisted,
    Public,
}

impl Privacy {
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Privacy::Private => "private",
            Privacy::Unlisted => "unlisted",
            Privacy::Public => "public",
        }
    }
}

/// Upload a video to YouTube using the resumable upload protocol.
#[derive(Debug, Parser)]
#[command(name = "yt-upload")]
pub struct Args {
    /// Path to the video file to upload
    pub file: PathBuf,

    /// Video title
    #[arg(long)]
    pub title: String,

    /// Video description
    #[arg(long, default_value = "")]
    pub description: String,

    /// Comma-separated tags
    #[arg(long, value_delimiter = ',', default_value = "")]
    pub tags: Vec<String>,

    /// Privacy status
    #[arg(long, value_enum, default_value_t = Privacy::Private)]
    pub privacy: Privacy,

    /// YouTube category ID
    #[arg(long, default_value_t = 22)]
    pub category: u32,
}
```

- [ ] **Step 4: Register the module and run tests**

In `src/main.rs`, add below `mod state;`:

```rust
mod cli;
```

Run: `cargo test --lib cli`
Expected: PASS. Note: `default_value = ""` combined with `value_delimiter` on an empty string yields one empty-string element, not zero — check the `tags_split_on_comma`/defaults test output; if `defaults_privacy_private_and_category_22` fails because `args.tags` is `[""]` instead of empty, change the field to `#[arg(long, value_delimiter = ',')] pub tags: Vec<String>` (no default_value) so an omitted flag yields an empty `Vec`, then re-run.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "Add CLI argument parsing"
```

---

### Task 5: Metadata JSON builder (`metadata.rs`)

**Files:**
- Create: `src/metadata.rs`
- Modify: `src/main.rs` (add `mod metadata;`)

- [ ] **Step 1: Write the failing tests**

Create `src/metadata.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Args, Privacy};
    use std::path::PathBuf;

    fn sample_args() -> Args {
        Args {
            file: PathBuf::from("video.mp4"),
            title: "My Title".to_string(),
            description: "My description".to_string(),
            tags: vec!["rust".to_string(), "youtube".to_string()],
            privacy: Privacy::Unlisted,
            category: 22,
        }
    }

    #[test]
    fn includes_title_description_and_tags() {
        let body = build_metadata(&sample_args());
        assert_eq!(body["snippet"]["title"], "My Title");
        assert_eq!(body["snippet"]["description"], "My description");
        assert_eq!(body["snippet"]["tags"][0], "rust");
        assert_eq!(body["snippet"]["categoryId"], "22");
    }

    #[test]
    fn includes_privacy_status() {
        let body = build_metadata(&sample_args());
        assert_eq!(body["status"]["privacyStatus"], "unlisted");
    }

    #[test]
    fn empty_tags_produce_empty_array() {
        let mut args = sample_args();
        args.tags = vec![];
        let body = build_metadata(&args);
        assert_eq!(body["snippet"]["tags"].as_array().unwrap().len(), 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib metadata`
Expected: FAIL — `build_metadata` is not defined yet.

- [ ] **Step 3: Implement metadata.rs above the test module**

Add this above the `#[cfg(test)] mod tests` block in `src/metadata.rs`:

```rust
use crate::cli::Args;
use serde_json::{json, Value};

pub fn build_metadata(args: &Args) -> Value {
    json!({
        "snippet": {
            "title": args.title,
            "description": args.description,
            "tags": args.tags,
            "categoryId": args.category.to_string(),
        },
        "status": {
            "privacyStatus": args.privacy.as_api_str(),
        }
    })
}
```

- [ ] **Step 4: Register the module and run tests**

In `src/main.rs`, add below `mod cli;`:

```rust
mod metadata;
```

Run: `cargo test --lib metadata`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/metadata.rs src/main.rs
git commit -m "Add YouTube API metadata JSON builder"
```

---

### Task 6: Auth — client secret, token cache, and pure parsing helpers (`auth.rs` part 1)

**Files:**
- Create: `src/auth.rs`
- Modify: `src/main.rs` (add `mod auth;`)

- [ ] **Step 1: Write the failing tests**

Create `src/auth.rs` with only the test module:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib auth`
Expected: FAIL — `build_authorize_url`, `load_client_secret`, `TokenCache`, `save_token_cache`, `load_token_cache`, `extract_code_from_request_line` are not defined yet.

- [ ] **Step 3: Implement the pure/testable parts of auth.rs above the test module**

Add this above the `#[cfg(test)] mod tests` block in `src/auth.rs`:

```rust
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
    let data = std::fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!("could not read client secret at {}: {e}", path.display())
    })?;
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
    }
    std::fs::write(path, serde_json::to_string_pretty(cache)?)
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
```

- [ ] **Step 4: Register the module and run tests**

In `src/main.rs`, add below `mod metadata;`:

```rust
mod auth;
```

Run: `cargo test --lib auth`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs src/main.rs
git commit -m "Add OAuth client secret, token cache, and redirect parsing"
```

---

### Task 7: Auth — live OAuth2 flow (`auth.rs` part 2)

This task adds the network- and browser-dependent parts of the flow. Per the
design spec, these are validated by manual smoke-testing (real browser
consent + real token exchange), not automated tests, since mocking Google's
OAuth endpoints would test the mock rather than real behavior.

**Files:**
- Modify: `src/auth.rs` (append below the existing implementation, still above `#[cfg(test)] mod tests`)

- [ ] **Step 1: Add the token response type and helpers**

Append to `src/auth.rs`, above the test module:

```rust
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
```

- [ ] **Step 2: Add code-for-token exchange and refresh**

Append to `src/auth.rs`, above the test module:

```rust
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
```

- [ ] **Step 3: Add the loopback listener that runs the installed-app flow**

Append to `src/auth.rs`, above the test module:

```rust
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
    let code = extract_code_from_request_line(&request)
        .ok_or_else(|| anyhow::anyhow!("no authorization code found in OAuth redirect"))?;

    let body = "Authorization received. You can close this tab and return to the terminal.";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;

    exchange_code_for_token(client, &code, &redirect_uri).await
}
```

- [ ] **Step 4: Add the top-level "get a valid access token" entry point**

Append to `src/auth.rs`, above the test module:

```rust
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
        let refreshed = refresh_access_token(client, &cache).await?;
        save_token_cache(cache_path, &refreshed)?;
        return Ok(refreshed.access_token);
    }

    let fresh = run_installed_app_flow(client).await?;
    save_token_cache(cache_path, &fresh)?;
    Ok(fresh.access_token)
}
```

- [ ] **Step 5: Verify the crate still builds and existing tests pass**

Run: `cargo build && cargo test --lib auth`
Expected: builds cleanly; the 6 tests from Task 6 still PASS (no new tests in this task — these functions require live network/browser interaction and are smoke-tested manually in Task 10).

- [ ] **Step 6: Commit**

```bash
git add src/auth.rs
git commit -m "Add live OAuth2 installed-app flow (token exchange, refresh, loopback listener)"
```

---

### Task 8: Resumable upload protocol (`resumable_upload.rs`)

**Files:**
- Create: `src/resumable_upload.rs`
- Modify: `src/main.rs` (add `mod resumable_upload;`)

- [ ] **Step 1: Write the failing tests for the one pure/parsing function**

Create `src/resumable_upload.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uploaded_bytes_from_range_header() {
        assert_eq!(parse_uploaded_bytes_from_range("bytes=0-999").unwrap(), 1000);
    }

    #[test]
    fn parses_uploaded_bytes_from_range_header_zero_based() {
        assert_eq!(parse_uploaded_bytes_from_range("bytes=0-0").unwrap(), 1);
    }

    #[test]
    fn rejects_malformed_range_header() {
        assert!(parse_uploaded_bytes_from_range("not-a-range").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib resumable_upload`
Expected: FAIL — `parse_uploaded_bytes_from_range` is not defined yet.

- [ ] **Step 3: Implement the full module above the test module**

Add this above the `#[cfg(test)] mod tests` block in `src/resumable_upload.rs`:

```rust
use crate::chunk::{next_chunk_range, ChunkRange, CHUNK_SIZE};
use crate::state::{self, UploadState};
use anyhow::{anyhow, Context};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const UPLOAD_ENDPOINT: &str =
    "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";

pub struct UploadResult {
    pub video_id: String,
    pub video_url: String,
}

enum ChunkOutcome {
    Incomplete(u64),
    Complete(String),
}

/// Parses a `Range: bytes=0-N` response header into the number of bytes
/// the server has confirmed receiving (N + 1).
fn parse_uploaded_bytes_from_range(range: &str) -> anyhow::Result<u64> {
    let end = range
        .strip_prefix("bytes=0-")
        .ok_or_else(|| anyhow!("unexpected Range header format: {range}"))?;
    Ok(end.parse::<u64>()? + 1)
}

async fn create_session(
    http: &reqwest::Client,
    access_token: &str,
    metadata: &Value,
) -> anyhow::Result<String> {
    let resp = http
        .post(UPLOAD_ENDPOINT)
        .bearer_auth(access_token)
        .header("X-Upload-Content-Type", "video/*")
        .json(metadata)
        .send()
        .await?
        .error_for_status()
        .context("creating resumable upload session")?;

    resp.headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("no Location header in session creation response"))
}

async fn query_uploaded_bytes(
    http: &reqwest::Client,
    session_uri: &str,
    total: u64,
) -> anyhow::Result<u64> {
    let resp = http
        .put(session_uri)
        .header("Content-Range", format!("bytes */{total}"))
        .header("Content-Length", "0")
        .send()
        .await?;

    match resp.status().as_u16() {
        308 => {
            let range = resp
                .headers()
                .get("Range")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("no Range header on 308 response"))?;
            parse_uploaded_bytes_from_range(range)
        }
        200 | 201 => Ok(total),
        other => Err(anyhow!("unexpected status {other} querying upload status")),
    }
}

async fn put_chunk_with_retry(
    http: &reqwest::Client,
    session_uri: &str,
    range: &ChunkRange,
    chunk: &[u8],
) -> anyhow::Result<ChunkOutcome> {
    const MAX_RETRIES: u32 = 5;
    let mut attempt = 0u32;

    loop {
        let resp = http
            .put(session_uri)
            .header("Content-Range", range.content_range_header())
            .header("Content-Length", chunk.len().to_string())
            .body(chunk.to_vec())
            .send()
            .await;

        match resp {
            Ok(r) if r.status().as_u16() == 308 => {
                return Ok(ChunkOutcome::Incomplete(range.end + 1));
            }
            Ok(r) if r.status().is_success() => {
                let body: Value = r.json().await?;
                let video_id = body["id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("no id in completed upload response"))?
                    .to_string();
                return Ok(ChunkOutcome::Complete(video_id));
            }
            Ok(r) if r.status().as_u16() == 404 => {
                return Err(anyhow!("upload session expired (404)"));
            }
            Ok(r) if r.status().is_client_error() => {
                let body = r.text().await.unwrap_or_default();
                return Err(anyhow!("upload rejected by API: {body}"));
            }
            Ok(_) | Err(_) => {
                attempt += 1;
                if attempt > MAX_RETRIES {
                    return Err(anyhow!("exceeded max retries uploading a chunk"));
                }
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
            }
        }
    }
}

pub async fn run(
    http: &reqwest::Client,
    access_token: &str,
    video_path: &Path,
    metadata: &Value,
    mut on_progress: impl FnMut(u64, u64),
) -> anyhow::Result<UploadResult> {
    let file_meta = std::fs::metadata(video_path).context("reading video file metadata")?;
    let file_size = file_meta.len();
    let mtime = file_meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let state_path = state::state_file_path(video_path);

    let (session_uri, mut uploaded) = match state::load_state(&state_path) {
        Some(existing) if state::matches_current_file(&existing, file_size, mtime) => {
            match query_uploaded_bytes(http, &existing.session_uri, file_size).await {
                Ok(bytes) => (existing.session_uri, bytes),
                Err(_) => {
                    state::delete_state(&state_path);
                    (create_session(http, access_token, metadata).await?, 0)
                }
            }
        }
        _ => {
            state::delete_state(&state_path);
            (create_session(http, access_token, metadata).await?, 0)
        }
    };

    state::save_state(
        &state_path,
        &UploadState {
            file_path: video_path.to_string_lossy().into_owned(),
            file_size,
            file_mtime: mtime,
            session_uri: session_uri.clone(),
        },
    )?;

    let bytes = tokio::fs::read(video_path)
        .await
        .context("reading video file into memory")?;

    while let Some(range) = next_chunk_range(uploaded, file_size, CHUNK_SIZE) {
        let chunk = &bytes[range.start as usize..=range.end as usize];
        match put_chunk_with_retry(http, &session_uri, &range, chunk).await? {
            ChunkOutcome::Incomplete(next_offset) => {
                uploaded = next_offset;
                on_progress(uploaded, file_size);
            }
            ChunkOutcome::Complete(video_id) => {
                state::delete_state(&state_path);
                return Ok(UploadResult {
                    video_url: format!("https://youtu.be/{video_id}"),
                    video_id,
                });
            }
        }
    }

    Err(anyhow!("upload loop ended without a completion response"))
}
```

- [ ] **Step 4: Register the module and run tests**

In `src/main.rs`, add below `mod auth;`:

```rust
mod resumable_upload;
```

Run: `cargo build && cargo test --lib resumable_upload`
Expected: builds cleanly; 3 tests PASS. (`run`, `create_session`, `query_uploaded_bytes`, and `put_chunk_with_retry` require a live API and are smoke-tested manually in Task 10, per the spec's testing decision.)

- [ ] **Step 5: Commit**

```bash
git add src/resumable_upload.rs src/main.rs
git commit -m "Add resumable upload protocol implementation"
```

---

### Task 9: Wire up main.rs

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace the placeholder main with the full wiring**

Replace the entire contents of `src/main.rs` with:

```rust
mod auth;
mod chunk;
mod cli;
mod metadata;
mod resumable_upload;
mod state;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    let client_secret = auth::load_client_secret(&auth::client_secret_path()).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nCreate an OAuth 'Desktop app' client in Google Cloud Console and save its JSON to {}",
            auth::client_secret_path().display()
        )
    })?;

    let access_token =
        auth::get_valid_access_token(&client_secret, &auth::token_cache_path()).await?;

    let http = reqwest::Client::new();
    let metadata = metadata::build_metadata(&args);

    let result = resumable_upload::run(
        &http,
        &access_token,
        &args.file,
        &metadata,
        |uploaded, total| {
            eprint!("\rUploaded {uploaded}/{total} bytes");
        },
    )
    .await?;

    eprintln!();
    println!("{}", result.video_url);

    Ok(())
}
```

- [ ] **Step 2: Verify the full crate builds and all unit tests still pass**

Run: `cargo build && cargo test`
Expected: builds cleanly; all unit tests from Tasks 2–8 PASS (28 tests total: 4 chunk + 7 state + 5 cli + 3 metadata + 6 auth + 3 resumable_upload). None fail.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "Wire up yt-upload CLI end to end"
```

---

### Task 10: README and manual smoke test

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write the README**

```markdown
# yt-upload

A small CLI that uploads a video to YouTube using the resumable upload
protocol.

## Setup

1. Create a Google Cloud project and enable the YouTube Data API v3.
2. Create an OAuth client ID of type **Desktop app**.
3. Download its JSON and save it to:
   - macOS/Linux: `~/.config/yt-upload/client_secret.json`
   - Windows: `%APPDATA%\yt-upload\client_secret.json`

## Usage

```bash
yt-upload video.mp4 --title "My Video" \
  --description "A description" \
  --tags rust,youtube \
  --privacy unlisted \
  --category 22
```

On first run, a browser window opens for you to authorize the app; the
resulting token is cached locally so later runs don't need to reauthorize.

If the upload is interrupted (network drop, process killed), re-running the
same command on the same file resumes from the last confirmed byte instead
of starting over.
```

- [ ] **Step 2: Build the release binary**

Run: `cargo build --release`
Expected: compiles successfully, producing `target/release/yt-upload`.

- [ ] **Step 3: Manual smoke test (requires real Google credentials)**

With a real `client_secret.json` in place and a small test video file:

Run: `./target/release/yt-upload test-video.mp4 --title "yt-upload smoke test" --privacy private`

Expected: browser opens for consent (first run only), progress is printed
to stderr as chunks upload, and a `https://youtu.be/...` URL is printed to
stdout on success. Verify the video appears in YouTube Studio as a private
upload, then delete the test video and (if desired) revoke the app's access
in your Google Account's third-party access settings.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "Add README with setup instructions"
```
