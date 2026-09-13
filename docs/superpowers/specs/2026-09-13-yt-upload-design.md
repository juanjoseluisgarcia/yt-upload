# yt-upload — Design Spec

## Purpose

A small, single-binary Rust CLI that uploads a video to YouTube using the
[resumable upload
protocol](https://developers.google.com/youtube/v3/guides/using_resumable_upload_protocol).
Covers OAuth2 authentication, chunked resumable upload with basic metadata,
and resume-after-interruption. Nothing beyond that (no thumbnails, playlists,
listing/editing/deleting videos, batch upload, or config files) is in scope
for v1.

## Language choice: Rust vs Go

Rust was chosen over Go for this project:

- **Binary size / footprint**: Rust produces a single static binary with no
  runtime; Go ships a runtime + GC. Both are "small" in absolute terms, but
  Rust binaries are typically smaller and have no GC pauses to worry about
  during long-running upload streams.
- **No concurrency advantage for Go here**: the resumable upload protocol is
  sequential chunked HTTP PUT/POST with byte-range tracking and retry logic —
  it doesn't benefit from goroutines/channels in a way Rust's async/await
  (via `tokio`) doesn't already cover.
- **Trade-off acknowledged**: Go's `net/http` is more ergonomic out of the
  box, and Go would mean less boilerplate. This is accepted in exchange for
  a smaller footprint and finer control over retry/backoff logic.

## Architecture

Single binary, five modules:

### 1. `auth`

Implements the OAuth2 "installed app" flow:

- On first run, opens the user's browser to Google's OAuth consent screen
  for the scope `https://www.googleapis.com/auth/youtube.upload`.
- Spins up a temporary localhost HTTP listener to catch the redirect
  containing the auth code (loopback flow — no need to paste a code
  manually).
- Exchanges the code for an access token + refresh token.
- Reads the OAuth client ID/secret from
  `~/.config/yt-upload/client_secret.json` (the "Desktop app" OAuth client
  JSON downloaded from Google Cloud Console — the user must create this
  themselves in their own GCP project).
- Caches the refresh token at `~/.config/yt-upload/token.json`.
- On subsequent runs, silently exchanges the refresh token for a new access
  token — no browser interaction needed unless the refresh token is revoked
  or missing.

### 2. `resumable_upload`

Implements the resumable upload protocol against the YouTube Data API v3
`videos.insert` endpoint:

- `POST` to `https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status`
  with the metadata JSON body (title, description, tags, category, privacy
  status). The response's `Location` header is the session URI.
- `PUT` the file to the session URI in chunks. Default chunk size 8 MiB
  (must be a multiple of 256 KiB per the API's requirement), with
  `Content-Range: bytes <start>-<end>/<total>` on each chunk.
- On `308 Resume Incomplete`, read the `Range` response header to determine
  how many bytes the server has received so far, and continue from there.
- On `200`/`201`, parse the returned video resource JSON to get the video ID
  and construct the watch URL for the final success message.
- Retry policy: exponential backoff on 5xx responses and connection-level
  errors. A `404` on a chunk PUT means the upload session itself expired —
  in that case, delete local state and restart the session from scratch
  (re-POST for a new session URI). Other 4xx errors are fatal and reported
  to the user.

### 3. `state`

Enables resuming an interrupted upload without restarting from byte 0:

- Before starting the chunked PUT loop, write a small JSON sidecar file
  under `~/.local/state/yt-upload/<sha256 of absolute file path>.json`
  (XDG state dir on Linux/macOS; platform equivalent on Windows via the
  `dirs` crate) containing: file path, file size, file mtime, and the
  session URI.
- On startup, before creating a new session, check whether a state file
  exists for this exact file path + size + mtime. If it does, perform a
  zero-byte `PUT` with `Content-Range: bytes */<size>` against the stored
  session URI to ask YouTube how many bytes it has already received, then
  resume the chunked PUT loop from that offset instead of starting a new
  session.
- If the stored session URI has expired (YouTube returns 404/410 on the
  status check), delete the state file and start a fresh session.
- Delete the state file on successful upload completion.

### 4. `cli`

`clap`-based argument parsing:

```
yt-upload <FILE> --title <TITLE>
          [--description <DESCRIPTION>]
          [--tags <a,b,c>]
          [--privacy private|unlisted|public]   (default: private)
          [--category <ID>]                      (default: 22, People & Blogs)
```

`--title` is required; all other metadata flags are optional with the
defaults above. No metadata file support in v1 (flags only, per user
decision).

### 5. `main`

Wires everything together:

1. Load or refresh OAuth credentials via `auth`.
2. Build the metadata JSON from parsed CLI flags.
3. Call `resumable_upload::run(...)`, which internally consults `state` for
   resume-in-progress logic.
4. Print upload progress (bytes uploaded / total, updated per chunk) to
   stderr.
5. On success, print the resulting video URL to stdout. On failure, print a
   clear error message (auth errors point the user at re-running to redo
   the OAuth flow; upload errors explain whether it's retryable state).

## Dependencies

- `reqwest` — HTTP client (streams file chunks without loading the whole
  file into memory).
- `tokio` — async runtime.
- `clap` — CLI argument parsing.
- `serde` / `serde_json` — API request/response payloads and state file
  (de)serialization.
- `dirs` — cross-platform config/state directory resolution.
- `sha2` — hashing file paths for state-file naming.

## Error handling

- Network/5xx errors during a chunk PUT: retried with exponential backoff
  (bounded number of retries before giving up and preserving state for a
  later re-run).
- 404 on a chunk PUT: session expired, state cleared, restart cleanly.
- Auth failures: clear message instructing the user to check
  `client_secret.json` or re-run to redo the browser consent flow.
- All other 4xx errors from the API (e.g. invalid category ID, quota
  exceeded): surfaced to the user verbatim from the API's error body, not
  swallowed.

## Testing

- Unit tests for chunk byte-range math (given file size + chunk size,
  correct `Content-Range` values) and for the state-file resume decision
  logic (given a state file + current file metadata, decide resume vs.
  fresh session).
- An integration test that performs a real upload against the live API is
  explicitly **not** included in the default test suite — it would require
  real credentials and pollute a real channel with test videos. Mocking the
  resumable protocol's chunk/308/retry behavior was considered but rejected:
  a hand-rolled mock is more likely to diverge from YouTube's actual
  behavior than to catch real bugs, so protocol correctness is validated
  against Google's documented behavior and via manual smoke-testing before
  release, not automated integration tests.

## Explicitly out of scope for v1

- Thumbnail upload
- Playlist management
- Listing, updating, or deleting existing videos
- Batch/multi-file upload
- A config file for default metadata values
- OS keychain credential storage (plain files in the XDG config dir are
  used instead, per user decision)
