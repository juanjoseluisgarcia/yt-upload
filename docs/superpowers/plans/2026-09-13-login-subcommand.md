# Login Subcommand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn OAuth from an implicit side effect of uploading into explicit `yt-upload login` / `logout` / `status` subcommands, with `upload` becoming its own subcommand that requires an existing session.

**Architecture:** `cli.rs` gains a `Cli { command: Command }` wrapper with `Command::{Upload, Login, Logout, Status}`; `auth.rs` gains a single private `valid_cached_token` helper that every read path (`get_cached_access_token`, `login`, `status`) wraps; `main.rs` dispatches on `Command` instead of always running an upload. Full design rationale lives in `docs/superpowers/specs/2026-09-13-login-subcommand-design.md`.

**Tech Stack:** Rust, clap (derive), tokio, reqwest, anyhow, serde.

---

Reference — current file contents this plan modifies, before any changes:
- `src/auth.rs` (310 lines) — see Task 1 onward for exact line ranges.
- `src/cli.rs` (97 lines) — see Task 8.
- `src/metadata.rs` (72 lines) — see Task 8.
- `src/main.rs` (43 lines) — see Task 8.
- `README.md` (42 lines) — see Task 10.

Run `cargo test <name>` for a single test, `cargo test` for the whole suite. All commands below assume the repo root as the working directory.

---

### Task 1: Widen the OAuth scope to include the account email

**Files:**
- Modify: `src/auth.rs:4` (the `SCOPE` constant)
- Modify: `src/auth.rs:249-255` (the existing scope test)

- [ ] **Step 1: Update the test first**

In `src/auth.rs`, replace the `build_authorize_url_contains_client_id_scope_and_redirect` test body:

```rust
    #[test]
    fn build_authorize_url_contains_client_id_scope_and_redirect() {
        let url = build_authorize_url("abc123", "http://127.0.0.1:9000");
        assert!(url.contains("client_id=abc123"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A9000"));
        assert!(url.contains("youtube.upload"));
        assert!(url.contains("userinfo.email"));
        assert!(url.contains("response_type=code"));
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test build_authorize_url_contains_client_id_scope_and_redirect`
Expected: FAIL — the old assertion `url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube.upload")` is gone, but the new `userinfo.email` assertion fails since `SCOPE` doesn't contain it yet.

- [ ] **Step 3: Widen the scope constant**

Replace line 4 of `src/auth.rs`:

```rust
pub const SCOPE: &str =
    "https://www.googleapis.com/auth/youtube.upload https://www.googleapis.com/auth/userinfo.email";
```

- [ ] **Step 4: Run it to see it pass**

Run: `cargo test build_authorize_url_contains_client_id_scope_and_redirect`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "Widen OAuth scope to include the account email"
```

---

### Task 2: Add `fetch_user_email`

**Files:**
- Modify: `src/auth.rs` (add after `refresh_access_token`, i.e. after line 167)

- [ ] **Step 1: Add the function**

Insert into `src/auth.rs`, directly after the closing brace of `refresh_access_token` (currently line 167):

```rust

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    email: Option<String>,
}

/// Resolves the email of the account an access token belongs to. Requires
/// the `userinfo.email` scope to have been granted at login time.
pub async fn fetch_user_email(access_token: &str) -> anyhow::Result<String> {
    let http = reqwest::Client::new();
    let info: UserInfoResponse = http
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    info.email
        .ok_or_else(|| anyhow::anyhow!("Google did not return an email address for this account"))
}
```

No unit test here: this is a live network call against Google's userinfo endpoint, and the existing codebase doesn't unit-test its other network-bound functions (`exchange_code_for_token`, `refresh_access_token`) either — see the spec's Testing section.

- [ ] **Step 2: Confirm it compiles**

Run: `cargo build`
Expected: builds cleanly (the function is unused for now — that's fine, it's wired up in Task 8; `cargo build` does not fail on unused-`pub`-item warnings, only `cargo clippy -- -D warnings` would, and we don't run that until the final task)

- [ ] **Step 3: Commit**

```bash
git add src/auth.rs
git commit -m "Add fetch_user_email for resolving the authorized account"
```

---

### Task 3: Add the private `valid_cached_token` helper

**Files:**
- Modify: `src/auth.rs` (add after `fetch_user_email`)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/auth.rs` (after `token_cache_save_and_load_roundtrip`):

```rust

    #[tokio::test]
    async fn valid_cached_token_returns_none_when_no_cache_exists() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let client = ClientSecret {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
        };

        assert!(valid_cached_token(&client, &cache_path).await.is_none());
    }

    #[tokio::test]
    async fn valid_cached_token_returns_cache_without_refreshing_when_not_near_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let client = ClientSecret {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
        };
        let cache = TokenCache {
            refresh_token: "r-1".to_string(),
            access_token: "a-1".to_string(),
            expires_at: now_unix() + 3600,
        };
        save_token_cache(&cache_path, &cache).unwrap();

        let result = valid_cached_token(&client, &cache_path).await;
        assert_eq!(result.unwrap().access_token, "a-1");
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test valid_cached_token`
Expected: FAIL with "cannot find function `valid_cached_token` in this scope"

- [ ] **Step 3: Implement it**

Add to `src/auth.rs`, after `fetch_user_email`:

```rust

/// Returns the cached session if it's usable right now: present, and
/// either not close to expiry or successfully refreshed (with the
/// refreshed token persisted back to `cache_path`). Returns `None` if
/// there's no cache, or the refresh token has been revoked/expired —
/// callers treat that the same as "not logged in".
async fn valid_cached_token(client: &ClientSecret, cache_path: &Path) -> Option<TokenCache> {
    let cache = load_token_cache(cache_path)?;
    if cache.expires_at > now_unix() + 60 {
        return Some(cache);
    }
    let refreshed = refresh_access_token(client, &cache).await.ok()?;
    save_token_cache(cache_path, &refreshed).ok()?;
    Some(refreshed)
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test valid_cached_token`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "Add valid_cached_token as the single source of truth for session validity"
```

---

### Task 4: Add `get_cached_access_token`

**Files:**
- Modify: `src/auth.rs` (add after `valid_cached_token`)

- [ ] **Step 1: Write the failing tests**

Add to the tests module:

```rust

    #[tokio::test]
    async fn get_cached_access_token_errors_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let client = ClientSecret {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
        };

        let result = get_cached_access_token(&client, &cache_path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("yt-upload login"));
    }

    #[tokio::test]
    async fn get_cached_access_token_returns_valid_cached_token() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let client = ClientSecret {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
        };
        let cache = TokenCache {
            refresh_token: "r-1".to_string(),
            access_token: "a-1".to_string(),
            expires_at: now_unix() + 3600,
        };
        save_token_cache(&cache_path, &cache).unwrap();

        let token = get_cached_access_token(&client, &cache_path).await.unwrap();
        assert_eq!(token, "a-1");
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test get_cached_access_token`
Expected: FAIL with "cannot find function `get_cached_access_token` in this scope"

- [ ] **Step 3: Implement it**

Add to `src/auth.rs`, after `valid_cached_token`:

```rust

/// Returns the currently cached access token, silently refreshing it if
/// it's close to expiry. Unlike the old always-interactive flow, this
/// never opens a browser — callers that want to start a new session use
/// `login` instead.
pub async fn get_cached_access_token(
    client: &ClientSecret,
    cache_path: &Path,
) -> anyhow::Result<String> {
    valid_cached_token(client, cache_path)
        .await
        .map(|cache| cache.access_token)
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run `yt-upload login` first."))
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test get_cached_access_token`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "Add get_cached_access_token for the non-interactive upload path"
```

---

### Task 5: Add `LoginOutcome` and `login`

**Files:**
- Modify: `src/auth.rs` (add after `get_cached_access_token`)

- [ ] **Step 1: Write the failing test**

Add to the tests module:

```rust

    #[tokio::test]
    async fn login_returns_already_logged_in_for_a_valid_cached_session() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let client = ClientSecret {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
        };
        let cache = TokenCache {
            refresh_token: "r-1".to_string(),
            access_token: "a-1".to_string(),
            expires_at: now_unix() + 3600,
        };
        save_token_cache(&cache_path, &cache).unwrap();

        let outcome = login(&client, &cache_path, false).await.unwrap();
        match outcome {
            LoginOutcome::AlreadyLoggedIn(c) => assert_eq!(c.access_token, "a-1"),
            LoginOutcome::LoggedIn(_) => panic!("expected AlreadyLoggedIn"),
        }
    }
```

This only exercises the non-interactive branch (a valid cache already
exists, `force: false`). The interactive branch calls
`run_installed_app_flow`, which opens a real browser and a loopback
listener — like that function's own existing tests (none), it isn't
unit-tested.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test login_returns_already_logged_in`
Expected: FAIL with "cannot find function `login` in this scope"

- [ ] **Step 3: Implement it**

Add to `src/auth.rs`, after `get_cached_access_token`:

```rust

#[derive(Debug)]
pub enum LoginOutcome {
    AlreadyLoggedIn(TokenCache),
    LoggedIn(TokenCache),
}

/// Authorizes this CLI with a Google account. Unless `force` is set,
/// reuses an already-valid cached session instead of opening the browser
/// again.
pub async fn login(
    client: &ClientSecret,
    cache_path: &Path,
    force: bool,
) -> anyhow::Result<LoginOutcome> {
    if !force {
        if let Some(cache) = valid_cached_token(client, cache_path).await {
            return Ok(LoginOutcome::AlreadyLoggedIn(cache));
        }
    }

    let fresh = run_installed_app_flow(client).await?;
    save_token_cache(cache_path, &fresh)?;
    Ok(LoginOutcome::LoggedIn(fresh))
}
```

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test login_returns_already_logged_in`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "Add login with an already-logged-in short circuit"
```

---

### Task 6: Add `logout`

**Files:**
- Modify: `src/auth.rs` (add after `login`)

- [ ] **Step 1: Write the failing tests**

Add to the tests module:

```rust

    #[test]
    fn logout_removes_an_existing_cache_and_reports_true() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let cache = TokenCache {
            refresh_token: "r-1".to_string(),
            access_token: "a-1".to_string(),
            expires_at: 1_700_000_000,
        };
        save_token_cache(&cache_path, &cache).unwrap();

        assert!(logout(&cache_path).unwrap());
        assert!(!cache_path.exists());
    }

    #[test]
    fn logout_reports_false_when_nothing_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");

        assert!(!logout(&cache_path).unwrap());
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test logout_`
Expected: FAIL with "cannot find function `logout` in this scope"

- [ ] **Step 3: Implement it**

Add to `src/auth.rs`, after `login`:

```rust

/// Deletes the cached credentials, if any (local only — this does not
/// revoke the refresh token with Google). Returns whether a cache
/// existed and was removed.
pub fn logout(cache_path: &Path) -> anyhow::Result<bool> {
    if cache_path.exists() {
        std::fs::remove_file(cache_path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test logout_`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "Add logout"
```

---

### Task 7: Add `status`, and remove the now-superseded `get_valid_access_token`

**Files:**
- Modify: `src/auth.rs` (delete `get_valid_access_token` and its doc comment — by
  this task it's no longer near its original lines 213-242, since Tasks
  2-6 inserted functions above it; find it by the code text in Step 3
  below, not by line number)
- Modify: `src/auth.rs` (add `status` after `logout`)

- [ ] **Step 1: Write the failing tests**

Add to the tests module:

```rust

    #[tokio::test]
    async fn status_returns_none_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let client = ClientSecret {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
        };

        assert!(status(&client, &cache_path).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn status_returns_cache_when_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("token.json");
        let client = ClientSecret {
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
        };
        let cache = TokenCache {
            refresh_token: "r-1".to_string(),
            access_token: "a-1".to_string(),
            expires_at: now_unix() + 3600,
        };
        save_token_cache(&cache_path, &cache).unwrap();

        let result = status(&client, &cache_path).await.unwrap();
        assert_eq!(result.unwrap().access_token, "a-1");
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test status_`
Expected: FAIL with "cannot find function `status` in this scope"

- [ ] **Step 3: Delete `get_valid_access_token`**

In `src/auth.rs`, delete these lines entirely (the doc comment and function, currently lines 213-242):

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
```

It's superseded by `get_cached_access_token` (non-interactive) and `login`
(interactive) — nothing calls it after this task (main.rs is rewired in
Task 8, which comes next; `cargo build` will show `main.rs:22` failing to
find `get_valid_access_token` until that task — that's expected and fixed
there. If you'd rather keep `cargo build` green at every step, do Task 8's
main.rs rewrite before deleting this function; either order reaches the
same end state).

- [ ] **Step 4: Add `status`**

Add to `src/auth.rs`, after `logout`:

```rust

/// Reports the currently authorized session, if any, silently
/// refreshing it first if it's close to expiry.
pub async fn status(
    client: &ClientSecret,
    cache_path: &Path,
) -> anyhow::Result<Option<TokenCache>> {
    Ok(valid_cached_token(client, cache_path).await)
}
```

- [ ] **Step 5: Run the new tests to see them pass**

Run: `cargo test status_`
Expected: PASS (2 tests) — note `cargo build` (the whole crate) still
fails at this point because `main.rs` still references the
now-deleted `get_valid_access_token`; that's fixed in Task 8, next.

- [ ] **Step 6: Commit**

```bash
git add src/auth.rs
git commit -m "Add status; remove get_valid_access_token, superseded by login/get_cached_access_token"
```

---

### Task 8: Restructure the CLI into `upload`/`login`/`logout`/`status` subcommands

**Files:**
- Modify: `src/cli.rs` (whole file)
- Modify: `src/metadata.rs:1,4,31-32` (rename `Args` to `UploadArgs`)
- Modify: `src/main.rs` (whole file)

This is the task where the crate goes from not-building (end of Task 7) to
building and all tests passing again — `cli.rs`, `metadata.rs`, and
`main.rs` are tightly coupled (all reference the `Args` type / auth
functions), so they're rewritten together in one task.

- [ ] **Step 1: Rewrite `src/cli.rs`**

Replace the entire contents of `src/cli.rs`:

```rust
use clap::{Parser, Subcommand, ValueEnum};
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

/// Upload videos to YouTube using the resumable upload protocol.
#[derive(Debug, Parser)]
#[command(name = "yt-upload")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Upload a video to YouTube using the resumable upload protocol
    Upload(UploadArgs),
    /// Authorize this CLI with a Google account
    Login {
        /// Re-run the browser consent flow even if already logged in
        #[arg(long)]
        force: bool,
    },
    /// Forget the cached credentials
    Logout,
    /// Show whether yt-upload is currently authorized
    Status,
}

#[derive(Debug, Parser)]
pub struct UploadArgs {
    /// Path to the video file to upload
    pub file: PathBuf,

    /// Video title
    #[arg(long)]
    pub title: String,

    /// Video description
    #[arg(long, default_value = "")]
    pub description: String,

    /// Comma-separated tags
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,

    /// Privacy status
    #[arg(long, value_enum, default_value_t = Privacy::Private)]
    pub privacy: Privacy,

    /// YouTube category ID
    #[arg(long, default_value_t = 22)]
    pub category: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload_args(cli: Cli) -> UploadArgs {
        match cli.command {
            Command::Upload(args) => args,
            other => panic!("expected Command::Upload, got {other:?}"),
        }
    }

    #[test]
    fn parses_required_file_and_title() {
        let cli =
            Cli::try_parse_from(["yt-upload", "upload", "video.mp4", "--title", "My Video"])
                .unwrap();
        let args = upload_args(cli);
        assert_eq!(args.file.to_str().unwrap(), "video.mp4");
        assert_eq!(args.title, "My Video");
    }

    #[test]
    fn defaults_privacy_private_and_category_22() {
        let cli =
            Cli::try_parse_from(["yt-upload", "upload", "video.mp4", "--title", "T"]).unwrap();
        let args = upload_args(cli);
        assert_eq!(args.privacy, Privacy::Private);
        assert_eq!(args.category, 22);
        assert_eq!(args.description, "");
        assert!(args.tags.is_empty());
    }

    #[test]
    fn tags_split_on_comma() {
        let cli = Cli::try_parse_from([
            "yt-upload", "upload", "video.mp4", "--title", "T", "--tags", "a,b,c",
        ])
        .unwrap();
        let args = upload_args(cli);
        assert_eq!(args.tags, vec!["a", "b", "c"]);
    }

    #[test]
    fn privacy_accepts_unlisted_and_public() {
        let cli = Cli::try_parse_from([
            "yt-upload",
            "upload",
            "video.mp4",
            "--title",
            "T",
            "--privacy",
            "unlisted",
        ])
        .unwrap();
        let args = upload_args(cli);
        assert_eq!(args.privacy, Privacy::Unlisted);
        assert_eq!(args.privacy.as_api_str(), "unlisted");
    }

    #[test]
    fn missing_title_is_an_error() {
        let result = Cli::try_parse_from(["yt-upload", "upload", "video.mp4"]);
        assert!(result.is_err());
    }

    #[test]
    fn login_parses_with_and_without_force() {
        let cli = Cli::try_parse_from(["yt-upload", "login"]).unwrap();
        assert!(matches!(cli.command, Command::Login { force: false }));

        let cli = Cli::try_parse_from(["yt-upload", "login", "--force"]).unwrap();
        assert!(matches!(cli.command, Command::Login { force: true }));
    }

    #[test]
    fn logout_and_status_parse() {
        let cli = Cli::try_parse_from(["yt-upload", "logout"]).unwrap();
        assert!(matches!(cli.command, Command::Logout));

        let cli = Cli::try_parse_from(["yt-upload", "status"]).unwrap();
        assert!(matches!(cli.command, Command::Status));
    }
}
```

Note `Command` needs `Debug` (already derived above) for the `{other:?}`
panic message in the `upload_args` test helper.

- [ ] **Step 2: Rename `Args` to `UploadArgs` in `src/metadata.rs`**

In `src/metadata.rs`, change line 1:

```rust
use crate::cli::UploadArgs;
```

Change line 4:

```rust
pub fn build_metadata(args: &UploadArgs) -> Value {
```

Change lines 31-32 (the `sample_args` test helper):

```rust
    fn sample_args() -> UploadArgs {
        UploadArgs {
```

(The rest of `metadata.rs` is unchanged — the field names on `UploadArgs`
are identical to the old `Args`.)

- [ ] **Step 3: Rewrite `src/main.rs`**

Replace the entire contents of `src/main.rs`:

```rust
mod auth;
mod chunk;
mod cli;
mod metadata;
mod resumable_upload;
mod state;

use clap::Parser;
use cli::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Command::Login { force } => {
            let client_secret = load_client_secret()?;
            let cache_path = auth::token_cache_path();
            match auth::login(&client_secret, &cache_path, force).await? {
                auth::LoginOutcome::AlreadyLoggedIn(cache) => {
                    let email = auth::fetch_user_email(&cache.access_token).await?;
                    println!("Already logged in as {email}. Use --force to re-authenticate.");
                }
                auth::LoginOutcome::LoggedIn(cache) => {
                    let email = auth::fetch_user_email(&cache.access_token).await?;
                    println!("Logged in as {email}.");
                }
            }
        }
        Command::Logout => {
            let cache_path = auth::token_cache_path();
            if auth::logout(&cache_path)? {
                println!("Logged out.");
            } else {
                println!("Not logged in.");
            }
        }
        Command::Status => {
            let client_secret = load_client_secret()?;
            let cache_path = auth::token_cache_path();
            match auth::status(&client_secret, &cache_path).await? {
                Some(cache) => {
                    let email = auth::fetch_user_email(&cache.access_token).await?;
                    println!("Logged in as {email}.");
                }
                None => println!("Not logged in. Run `yt-upload login`."),
            }
        }
        Command::Upload(args) => {
            let client_secret = load_client_secret()?;
            let cache_path = auth::token_cache_path();
            let access_token = auth::get_cached_access_token(&client_secret, &cache_path).await?;

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
            eprintln!("Video ID: {}", result.video_id);
            println!("{}", result.video_url);
        }
    }

    Ok(())
}

fn load_client_secret() -> anyhow::Result<auth::ClientSecret> {
    auth::load_client_secret(&auth::client_secret_path()).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nCreate an OAuth 'Desktop app' client in Google Cloud Console and save its JSON to {}",
            auth::client_secret_path().display()
        )
    })
}
```

- [ ] **Step 4: Build and run the whole test suite**

Run: `cargo build && cargo test`
Expected: builds cleanly, all tests pass (the `cli.rs` and `metadata.rs`
tests from earlier commits, plus every `auth.rs` test from Tasks 1-7).

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs src/metadata.rs src/main.rs
git commit -m "Restructure CLI into upload/login/logout/status subcommands"
```

---

### Task 9: Update the README

**Files:**
- Modify: `README.md:22-37` (the `## Usage` section)

- [ ] **Step 1: Replace the Usage section**

In `README.md`, replace everything from `## Usage` (line 22) through the
paragraph ending "...of starting over." (line 37) with:

```markdown
## Usage

Authorize the CLI once:

```bash
yt-upload login
```

This opens a browser window for you to authorize the app; the resulting
token is cached locally so later commands don't need to reauthorize. Run
`yt-upload login --force` to switch accounts or re-authorize explicitly.

Check whether you're currently authorized, and as which account:

```bash
yt-upload status
```

Forget the cached credentials:

```bash
yt-upload logout
```

Upload a video:

```bash
yt-upload upload video.mp4 --title "My Video" \
  --description "A description" \
  --tags rust,youtube \
  --privacy unlisted \
  --category 22
```

`upload` requires an existing session — if none exists it errors out with
a pointer to run `yt-upload login`, rather than opening a browser
mid-upload.

If the upload is interrupted (network drop, process killed), re-running
the same command on the same file resumes from the last confirmed byte
instead of starting over.

Run `yt-upload --help` (or `yt-upload <command> --help`) for the full list
of flags for any command.
```

- [ ] **Step 2: Sanity-check the rendered help matches what's documented**

Run: `cargo run -- --help` then `cargo run -- upload --help` then
`cargo run -- login --help`
Expected: the subcommand list and flags shown match what the README now
describes (this is a manual read-through, not an automated check — there's
no snapshot test for `--help` output in this project).

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "Update README for the login/logout/status/upload subcommands"
```

---

### Task 10: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full check**

Run: `make check`
Expected: `cargo fmt --check` passes, `cargo clippy --all-targets --all-features -- -D warnings` passes with no warnings, and `cargo test` passes (all tests, including every test added in Tasks 1-8).

- [ ] **Step 2: Fix anything `make check` surfaces**

If `cargo fmt --check` fails, run `cargo fmt` and review the diff (it
should be whitespace-only). If `clippy` flags anything (e.g. an unused
import), fix it directly — there should be nothing structural left to
change at this point.

- [ ] **Step 3: Manually exercise the new commands once**

With a real `client_secret.json` in place (see README Setup):

```bash
cargo run -- status   # expect: "Not logged in. Run `yt-upload login`."
cargo run -- login    # opens a browser; approve; expect: "Logged in as <email>."
cargo run -- login    # expect: "Already logged in as <email>. Use --force to re-authenticate."
cargo run -- status   # expect: "Logged in as <email>."
cargo run -- logout   # expect: "Logged out."
cargo run -- status   # expect: "Not logged in. Run `yt-upload login`."
```

- [ ] **Step 4: Commit if Step 2 required any fixes**

```bash
git add -A
git commit -m "Fix fmt/clippy findings from final verification"
```

(Skip this step entirely if `make check` was already clean in Step 1.)
