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
