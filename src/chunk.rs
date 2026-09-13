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
