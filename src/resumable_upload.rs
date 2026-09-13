use crate::chunk::{next_chunk_range, ChunkRange, CHUNK_SIZE};
use crate::state::{self, UploadState};
use anyhow::{anyhow, Context};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const UPLOAD_ENDPOINT: &str =
    "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";

/// Fingerprints the video metadata (title, description, tags, category,
/// privacy) so a resumed upload can detect that it changed since the
/// in-progress session was created. `serde_json`'s default `Map` is
/// backed by a `BTreeMap` (the `preserve_order` feature isn't enabled),
/// so key order — and therefore this hash — is stable for equal content
/// regardless of construction order.
fn hash_metadata(metadata: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(metadata).expect("Value always serializes"));
    hex::encode(hasher.finalize())
}

pub struct UploadResult {
    pub video_id: String,
    pub video_url: String,
}

enum ChunkOutcome {
    Incomplete(u64),
    Complete(String),
}

/// Marker error indicating the upload session URI is no longer valid
/// (the server returned 404 or 410), distinct from transient failures
/// like network errors or 5xx responses. Callers use this to decide
/// whether to discard resume state and start a fresh session, versus
/// propagating the error and preserving state for a retry.
#[derive(Debug)]
struct SessionExpired;

impl std::fmt::Display for SessionExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "upload session expired")
    }
}

impl std::error::Error for SessionExpired {}

fn is_session_expired(err: &anyhow::Error) -> bool {
    err.downcast_ref::<SessionExpired>().is_some()
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
    access_token: &str,
    session_uri: &str,
    total: u64,
) -> anyhow::Result<ChunkOutcome> {
    let resp = http
        .put(session_uri)
        .bearer_auth(access_token)
        .header("Content-Range", format!("bytes */{total}"))
        .header("Content-Length", "0")
        .send()
        .await?;

    match resp.status().as_u16() {
        // A 308 with no Range header means the server has received zero
        // bytes so far — the header is only present once at least one
        // byte has been confirmed. See Google's resumable upload docs:
        // "If the upload has not started, ... the response ... will not
        // include a Range header".
        308 => {
            let uploaded = resp
                .headers()
                .get("Range")
                .and_then(|v| v.to_str().ok())
                .map(parse_uploaded_bytes_from_range)
                .transpose()?
                .unwrap_or(0);
            Ok(ChunkOutcome::Incomplete(uploaded))
        }
        200 | 201 => {
            let body: Value = resp.json().await?;
            let video_id = body["id"]
                .as_str()
                .ok_or_else(|| anyhow!("no id in completed upload response"))?
                .to_string();
            Ok(ChunkOutcome::Complete(video_id))
        }
        404 | 410 => Err(anyhow!(SessionExpired)),
        other => Err(anyhow!("unexpected status {other} querying upload status")),
    }
}

async fn put_chunk_with_retry(
    http: &reqwest::Client,
    access_token: &str,
    session_uri: &str,
    range: &ChunkRange,
    chunk: &[u8],
) -> anyhow::Result<ChunkOutcome> {
    const MAX_RETRIES: u32 = 5;
    let mut attempt = 0u32;

    loop {
        let resp = http
            .put(session_uri)
            .bearer_auth(access_token)
            .header("Content-Range", range.content_range_header())
            .header("Content-Length", chunk.len().to_string())
            .body(chunk.to_vec())
            .send()
            .await;

        match resp {
            Ok(r) if r.status().as_u16() == 308 => {
                // The Range header is authoritative: the server may have
                // accepted fewer bytes than we sent. Fall back to assuming
                // the whole chunk was accepted only if the header is absent.
                let next_offset = r
                    .headers()
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| parse_uploaded_bytes_from_range(s).ok())
                    .unwrap_or(range.end + 1);
                return Ok(ChunkOutcome::Incomplete(next_offset));
            }
            Ok(r) if r.status().is_success() => {
                let body: Value = r.json().await?;
                let video_id = body["id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("no id in completed upload response"))?
                    .to_string();
                return Ok(ChunkOutcome::Complete(video_id));
            }
            Ok(r) if r.status().as_u16() == 404 || r.status().as_u16() == 410 => {
                return Err(anyhow!(SessionExpired));
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

                // Don't blindly resend the same bytes: a dropped response
                // after a successful write is indistinguishable, from this
                // client's point of view, from a write that never landed.
                // Ask the server what it authoritatively has before
                // retrying, and only resend this exact range if the server
                // confirms none of it arrived.
                match query_uploaded_bytes(http, access_token, session_uri, range.total).await {
                    Ok(ChunkOutcome::Incomplete(server_offset)) if server_offset != range.start => {
                        return Ok(ChunkOutcome::Incomplete(server_offset));
                    }
                    Ok(ChunkOutcome::Incomplete(_)) => {
                        // Confirmed nothing from this chunk landed; retry as planned.
                    }
                    Ok(ChunkOutcome::Complete(video_id)) => {
                        return Ok(ChunkOutcome::Complete(video_id));
                    }
                    Err(e) if is_session_expired(&e) => return Err(e),
                    Err(_) => {
                        // The status check itself failed (e.g. still
                        // offline); fall through and retry the chunk PUT.
                    }
                }
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

    if file_size == 0 {
        return Err(anyhow!(
            "cannot upload an empty file: {}",
            video_path.display()
        ));
    }

    let mtime = file_meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let state_path = state::state_file_path(video_path);
    let metadata_hash = hash_metadata(metadata);

    // A session's metadata (title, description, tags, category, privacy)
    // is fixed at creation time by the YouTube API — it can't be changed
    // on an in-progress session. So a metadata change is treated the same
    // as a file change: the stale session is discarded and a fresh one is
    // created with the new metadata, rather than silently resuming under
    // the old values.
    let (mut session_uri, mut uploaded) = match state::load_state(&state_path) {
        Some(existing)
            if state::matches_current_file(&existing, file_size, mtime)
                && existing.metadata_hash == metadata_hash =>
        {
            match query_uploaded_bytes(http, access_token, &existing.session_uri, file_size).await {
                Ok(ChunkOutcome::Incomplete(bytes)) => (existing.session_uri, bytes),
                Ok(ChunkOutcome::Complete(video_id)) => {
                    state::delete_state(&state_path);
                    return Ok(UploadResult {
                        video_url: format!("https://youtu.be/{video_id}"),
                        video_id,
                    });
                }
                Err(e) if is_session_expired(&e) => {
                    state::delete_state(&state_path);
                    (create_session(http, access_token, metadata).await?, 0)
                }
                Err(e) => return Err(e),
            }
        }
        _ => {
            state::delete_state(&state_path);
            (create_session(http, access_token, metadata).await?, 0)
        }
    };

    let save_current_state = |state_path: &Path, session_uri: &str| {
        state::save_state(
            state_path,
            &UploadState {
                file_path: video_path.to_string_lossy().into_owned(),
                file_size,
                file_mtime: mtime,
                session_uri: session_uri.to_string(),
                metadata_hash: metadata_hash.clone(),
            },
        )
    };
    save_current_state(&state_path, &session_uri)?;

    let mut file = tokio::fs::File::open(video_path)
        .await
        .context("opening video file")?;
    let mut buf = vec![0u8; CHUNK_SIZE as usize];

    while let Some(range) = next_chunk_range(uploaded, file_size, CHUNK_SIZE) {
        let len = range.len() as usize;
        file.seek(std::io::SeekFrom::Start(range.start))
            .await
            .context("seeking within video file")?;
        file.read_exact(&mut buf[..len])
            .await
            .context("reading chunk from video file")?;
        let chunk = &buf[..len];

        match put_chunk_with_retry(http, access_token, &session_uri, &range, chunk).await {
            Ok(ChunkOutcome::Incomplete(next_offset)) => {
                uploaded = next_offset;
                on_progress(uploaded, file_size);
            }
            Ok(ChunkOutcome::Complete(video_id)) => {
                state::delete_state(&state_path);
                return Ok(UploadResult {
                    video_url: format!("https://youtu.be/{video_id}"),
                    video_id,
                });
            }
            Err(e) if is_session_expired(&e) => {
                // The session died mid-upload (e.g. it outlived the
                // ~24h server-side TTL on a very large or slow upload).
                // Rather than aborting and making the caller re-run the
                // command, start a fresh session and keep going within
                // this same call.
                state::delete_state(&state_path);
                session_uri = create_session(http, access_token, metadata).await?;
                uploaded = 0;
                save_current_state(&state_path, &session_uri)?;
            }
            Err(e) => return Err(e),
        }
    }

    Err(anyhow!("upload loop ended without a completion response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uploaded_bytes_from_range_header() {
        assert_eq!(
            parse_uploaded_bytes_from_range("bytes=0-999").unwrap(),
            1000
        );
    }

    #[test]
    fn parses_uploaded_bytes_from_range_header_zero_based() {
        assert_eq!(parse_uploaded_bytes_from_range("bytes=0-0").unwrap(), 1);
    }

    #[test]
    fn rejects_malformed_range_header() {
        assert!(parse_uploaded_bytes_from_range("not-a-range").is_err());
    }

    #[test]
    fn hash_metadata_is_deterministic() {
        let metadata = serde_json::json!({"snippet": {"title": "T"}});
        assert_eq!(hash_metadata(&metadata), hash_metadata(&metadata));
    }

    #[test]
    fn hash_metadata_differs_for_different_content() {
        let a = serde_json::json!({"snippet": {"title": "A"}});
        let b = serde_json::json!({"snippet": {"title": "B"}});
        assert_ne!(hash_metadata(&a), hash_metadata(&b));
    }

    #[test]
    fn hash_metadata_is_stable_regardless_of_key_construction_order() {
        let a =
            serde_json::json!({"snippet": {"title": "T"}, "status": {"privacyStatus": "private"}});
        let b =
            serde_json::json!({"status": {"privacyStatus": "private"}, "snippet": {"title": "T"}});
        assert_eq!(hash_metadata(&a), hash_metadata(&b));
    }

    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn query_uploaded_bytes_treats_missing_range_as_zero_on_308() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(308))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let result = query_uploaded_bytes(&http, "token", &server.uri(), 1000)
            .await
            .unwrap();
        match result {
            ChunkOutcome::Incomplete(bytes) => assert_eq!(bytes, 0),
            ChunkOutcome::Complete(_) => panic!("expected Incomplete"),
        }
    }

    #[tokio::test]
    async fn query_uploaded_bytes_reads_range_header_when_present() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(308).insert_header("Range", "bytes=0-999"))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let result = query_uploaded_bytes(&http, "token", &server.uri(), 2000)
            .await
            .unwrap();
        match result {
            ChunkOutcome::Incomplete(bytes) => assert_eq!(bytes, 1000),
            ChunkOutcome::Complete(_) => panic!("expected Incomplete"),
        }
    }

    #[tokio::test]
    async fn put_chunk_with_retry_requeries_offset_instead_of_blindly_resending() {
        let server = MockServer::start().await;

        // The chunk PUT itself (Content-Length: 10) fails transiently.
        Mock::given(method("PUT"))
            .and(header("Content-Length", "10"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        // The status-check PUT (Content-Length: 0) that follows reports the
        // server actually has MORE bytes than this chunk's range covers —
        // e.g. the prior response was lost after the write succeeded. The
        // fix must trust this authoritative offset rather than blindly
        // resending the original 10-byte chunk.
        Mock::given(method("PUT"))
            .and(header("Content-Length", "0"))
            .respond_with(ResponseTemplate::new(308).insert_header("Range", "bytes=0-19"))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let range = ChunkRange {
            start: 0,
            end: 9,
            total: 1000,
        };
        let chunk = vec![0u8; 10];

        let result = put_chunk_with_retry(&http, "token", &server.uri(), &range, &chunk)
            .await
            .unwrap();
        match result {
            ChunkOutcome::Incomplete(offset) => assert_eq!(offset, 20),
            ChunkOutcome::Complete(_) => panic!("expected Incomplete"),
        }
    }
}
