mod auth;
mod chunk;
mod cli;
mod metadata;
mod resumable_upload;
mod state;

use clap::Parser;
use cli::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Command::Login { force } => {
            let client_secret = load_client_secret()?;
            let cache_path = auth::token_cache_path();
            match auth::login(&client_secret, &cache_path, force).await? {
                auth::LoginOutcome::AlreadyLoggedIn(cache) => {
                    let email = auth::fetch_user_email(&cache.access_token).await?;
                    println!("Already logged in as {email}. Use --force to re-authenticate.");
                }
                auth::LoginOutcome::LoggedIn(cache) => {
                    let email = auth::fetch_user_email(&cache.access_token).await?;
                    println!("Logged in as {email}.");
                }
            }
        }
        Command::Logout => {
            let cache_path = auth::token_cache_path();
            if auth::logout(&cache_path)? {
                println!("Logged out.");
            } else {
                println!("Not logged in.");
            }
        }
        Command::Status => {
            let client_secret = load_client_secret()?;
            let cache_path = auth::token_cache_path();
            match auth::status(&client_secret, &cache_path).await? {
                Some(cache) => {
                    let email = auth::fetch_user_email(&cache.access_token).await?;
                    println!("Logged in as {email}.");
                }
                None => println!("Not logged in. Run `yt-upload login`."),
            }
        }
        Command::Upload(args) => {
            let client_secret = load_client_secret()?;
            let cache_path = auth::token_cache_path();
            let access_token = auth::get_cached_access_token(&client_secret, &cache_path).await?;

            let http = reqwest::Client::new();
            let metadata = metadata::build_metadata(&args);

            let result = resumable_upload::run(
                &http,
                &access_token,
                &args.file,
                &metadata,
                |uploaded, total| {
                    eprint!("\rUploaded {uploaded}/{total} bytes");
                },
            )
            .await?;

            eprintln!();
            eprintln!("Video ID: {}", result.video_id);
            println!("{}", result.video_url);
        }
    }

    Ok(())
}

fn load_client_secret() -> anyhow::Result<auth::ClientSecret> {
    auth::load_client_secret(&auth::client_secret_path()).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nCreate an OAuth 'Desktop app' client in Google Cloud Console and save its JSON to {}",
            auth::client_secret_path().display()
        )
    })
}
