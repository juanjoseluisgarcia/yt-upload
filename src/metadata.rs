use crate::cli::UploadArgs;
use serde_json::{json, Value};

pub fn build_metadata(args: &UploadArgs) -> Value {
    let tags: Vec<&str> = args
        .tags
        .iter()
        .map(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .collect();

    json!({
        "snippet": {
            "title": args.title,
            "description": args.description,
            "tags": tags,
            "categoryId": args.category.to_string(),
        },
        "status": {
            "privacyStatus": args.privacy.as_api_str(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Privacy;
    use std::path::PathBuf;

    fn sample_args() -> UploadArgs {
        UploadArgs {
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

    #[test]
    fn empty_string_tags_are_filtered_out() {
        let mut args = sample_args();
        args.tags = vec!["".to_string()];
        let body = build_metadata(&args);
        assert_eq!(body["snippet"]["tags"].as_array().unwrap().len(), 0);
    }
}
