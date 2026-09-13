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
