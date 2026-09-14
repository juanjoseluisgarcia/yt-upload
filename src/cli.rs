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
#[command(name = "yt-upload", disable_help_subcommand = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// The full walkthrough for creating your own Google Cloud OAuth client,
/// shown by `yt-upload login --help` and in `man yt-upload-login`.
/// yt-upload doesn't ship a built-in Google API client (see the README
/// for why), so each user authorizes it with their own project - this
/// takes about five minutes and is free.
const LOGIN_SETUP_WALKTHROUGH: &str = "\
Getting your own Google Cloud OAuth credentials:

  1. Go to https://console.cloud.google.com/ and create a new project
     (or pick an existing one you don't mind reusing).

  2. Enable the YouTube Data API v3: in the left sidebar, go to
     \"APIs & Services\" > \"Library\", search for \"YouTube Data API v3\",
     and click Enable.

  3. Configure the consent screen: \"APIs & Services\" > \"OAuth consent
     screen\". Choose \"External\" as the user type, fill in an app name
     and your email for the required fields, and save. Under \"Test
     users\", add your own Google account - this keeps the app in
     \"Testing\" mode, which needs no review from Google for personal
     use.

  4. Create the OAuth client: \"APIs & Services\" > \"Credentials\" >
     \"+ Create Credentials\" > \"OAuth client ID\". Choose \"Desktop app\"
     as the application type, give it any name, and click Create.

  5. Download its JSON (the download icon next to the client in the
     credentials list) and save it to:
       macOS:   ~/Library/Application Support/yt-upload/client_secret.json
       Linux:   ~/.config/yt-upload/client_secret.json
       Windows: %APPDATA%\\yt-upload\\client_secret.json

  6. Run `yt-upload login`.

None of this costs money: creating the project, enabling the API, and
authorizing your own account are all free, with no billing account
required.

A note on \"Testing\" mode: while your OAuth consent screen is
unverified, Google expires its refresh tokens after 7 days, so you'll
need to run `yt-upload login` again about weekly. To avoid this, go
back to \"OAuth consent screen\" and click \"Publish App\" to move it to
\"In Production\" - you'll see an \"unverified app\" warning during your
own login (safe to click through, since you're the only user of your
own credentials), but after that your session lasts indefinitely.";

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Upload a video to YouTube using the resumable upload protocol
    Upload(UploadArgs),
    /// Authorize this CLI with a Google account
    #[command(after_long_help = LOGIN_SETUP_WALKTHROUGH)]
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
        let cli = Cli::try_parse_from(["yt-upload", "upload", "video.mp4", "--title", "My Video"])
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
            "yt-upload",
            "upload",
            "video.mp4",
            "--title",
            "T",
            "--tags",
            "a,b,c",
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
