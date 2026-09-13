# First-class login subcommand

## Problem

Authentication is currently an implicit side effect of running an upload:
`main()` unconditionally calls `auth::get_valid_access_token`, which opens a
browser mid-upload if there's no cached token or the refresh token was
revoked. There's no way to authenticate ahead of time, check auth state, or
clear cached credentials without deleting `token.json` by hand.

## Goals

- `yt-upload login` — explicit, first-class authorization.
- `yt-upload logout` — forget cached credentials (local only; does not
  revoke the token with Google).
- `yt-upload status` — report whether the CLI is currently authorized, and
  as which account.
- `yt-upload upload ...` — uploading requires a valid cached session; if
  none exists it errors with a pointer to `yt-upload login` instead of
  silently opening a browser mid-upload.

This is a breaking CLI change: uploads currently invoked as
`yt-upload video.mp4 --title X` become `yt-upload upload video.mp4 --title X`.

## CLI shape (`cli.rs`)

```rust
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
    // unchanged: file, title, description, tags, privacy, category
}
```

Doc comments on each variant/field are what clap surfaces in `--help`
output, so they must stay accurate — this is the only "help text" source
in the project (no separate man page).

## auth.rs changes

- **Scope**: `SCOPE` becomes a space-separated pair —
  `https://www.googleapis.com/auth/youtube.upload` plus
  `https://www.googleapis.com/auth/userinfo.email` — so a fetched access
  token can also resolve an account email.
- **`fetch_user_email(access_token: &str) -> anyhow::Result<String>`**: GET
  `https://www.googleapis.com/oauth2/v2/userinfo` with a bearer token,
  return the `email` field or error if Google didn't send one.
- **`valid_cached_token(client, cache_path) -> Option<TokenCache>`**
  (private): loads the cache; returns it as-is if not near expiry;
  otherwise attempts a silent refresh and re-saves on success. Returns
  `None` if there's no cache, or the refresh fails (revoked/expired
  refresh token). This is the single place that owns "is there a usable
  session right now" — every other function below is a thin wrapper over
  it, so there is exactly one refresh code path.
- **`get_cached_access_token(client, cache_path) -> anyhow::Result<String>`**:
  used by `upload`. `valid_cached_token(...).map(|c| c.access_token)` or a
  hard error: `"Not logged in. Run \`yt-upload login\` first."` No browser
  fallback — this replaces `get_valid_access_token`, which is removed.
- **`LoginOutcome`** enum: `AlreadyLoggedIn(TokenCache) | LoggedIn(TokenCache)`.
- **`login(client, cache_path, force: bool) -> anyhow::Result<LoginOutcome>`**:
  unless `force`, checks `valid_cached_token` first and returns
  `AlreadyLoggedIn` if it finds one; otherwise runs
  `run_installed_app_flow` (existing function, unchanged), saves the
  result, and returns `LoggedIn`.
- **`logout(cache_path) -> anyhow::Result<bool>`**: removes `token.json` if
  present; return value says whether there was anything to remove.
- **`status(client, cache_path) -> anyhow::Result<Option<TokenCache>>`**:
  `valid_cached_token(...)` — `Some` means logged in (and carries the
  token used to resolve the email), `None` means not logged in.

## main.rs dispatch

`client_secret.json` is loaded per-branch (not unconditionally at the top),
since `logout` needs neither it nor any network access.

```
Login { force } => login(); print "Logged in as <email>."
                   or "Already logged in as <email>. Use --force to re-authenticate."
Logout           => logout(); print "Logged out." or "Not logged in."
Status           => status(); print "Logged in as <email>."
                    or "Not logged in. Run `yt-upload login`."
Upload(args)     => get_cached_access_token() (errors out with the
                    login hint if not authorized), then today's upload
                    flow unchanged.
```

## Error handling

Unchanged style: functions return `anyhow::Result`, `main` propagates with
`?`, the default Rust runtime prints the error's Debug output on exit.
No new error type is introduced — the "not logged in" case is a plain
`anyhow!` message with an actionable hint, since (unlike the upload
protocol's `SessionExpired`) nothing downstream needs to distinguish it
programmatically.

## Testing

- `cli.rs`: existing tests reparsed through the new subcommands (e.g.
  `Cli::try_parse_from(["yt-upload", "upload", "video.mp4", "--title", "T"])`),
  plus new tests for `login --force`, `logout`, and `status` parsing.
- `auth.rs`: a test for `logout()`'s file-removal behavior (present vs.
  absent cache), and a test for `valid_cached_token`'s fast path (cache
  present, not near expiry — no network involved). Network-bound paths
  (`fetch_user_email`, token refresh, the browser flow) stay untested by
  unit tests, matching how `exchange_code_for_token`/`refresh_access_token`
  are handled today.

## Documentation

- `README.md`: usage section updated for `yt-upload upload ...`, plus a
  new short section documenting `login`/`logout`/`status`.
- `--help` output is generated by clap from the doc comments above — kept
  accurate as part of implementing the CLI shape, not as a separate task.
