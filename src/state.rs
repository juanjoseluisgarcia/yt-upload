use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadState {
    pub file_path: String,
    pub file_size: u64,
    pub file_mtime: u64,
    pub session_uri: String,
    /// Fingerprint of the video metadata (title, description, tags,
    /// category, privacy) in effect when this session was created, so a
    /// resume can detect that the caller's metadata has since changed.
    pub metadata_hash: String,
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
        restrict_dir_permissions(parent)?;
    }
    let data = serde_json::to_string_pretty(state)?;
    std::fs::write(state_path, data)
}

/// Restricts a directory's Unix permission bits to owner-only (0700). No-op
/// on non-Unix platforms, since this crate only documents macOS/Linux
/// support.
#[cfg(unix)]
fn restrict_dir_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub fn delete_state(state_path: &Path) {
    let _ = std::fs::remove_file(state_path);
}

/// Whether a previously stored session can be resumed for the file as it
/// currently exists on disk (same size and mtime), or should be discarded.
pub fn matches_current_file(state: &UploadState, current_size: u64, current_mtime: u64) -> bool {
    state.file_size == current_size && state.file_mtime == current_mtime
}

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
            metadata_hash: "hash".to_string(),
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
            metadata_hash: "hash".to_string(),
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
            metadata_hash: "hash".to_string(),
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
            metadata_hash: "hash".to_string(),
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
            metadata_hash: "hash".to_string(),
        };
        assert!(!matches_current_file(&state, 100, 999));
    }

    fn state_file_path_for_key(key: &str) -> PathBuf {
        hash_key_to_path(key)
    }
}
