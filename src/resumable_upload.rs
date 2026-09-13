use crate::chunk::{next_chunk_range, ChunkRange, CHUNK_SIZE};
use crate::state::{self, UploadState};
use anyhow::{anyhow, Context};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

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
        308 => {
            let range = resp
                .headers()
                .get("Range")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("no Range header on 308 response"))?;
            Ok(ChunkOutcome::Incomplete(parse_uploaded_bytes_from_range(
                range,
            )?))
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
        return Err(anyhow!("cannot upload an empty file: {}", video_path.display()));
    }

    let mtime = file_meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let state_path = state::state_file_path(video_path);

    let (session_uri, mut uploaded) = match state::load_state(&state_path) {
        Some(existing) if state::matches_current_file(&existing, file_size, mtime) => {
            match query_uploaded_bytes(http, access_token, &existing.session_uri, file_size).await
            {
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

    state::save_state(
        &state_path,
        &UploadState {
            file_path: video_path.to_string_lossy().into_owned(),
            file_size,
            file_mtime: mtime,
            session_uri: session_uri.clone(),
        },
    )?;

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
            Err(e) => {
                if is_session_expired(&e) {
                    state::delete_state(&state_path);
                }
                return Err(e);
            }
        }
    }

    Err(anyhow!("upload loop ended without a completion response"))
}

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
