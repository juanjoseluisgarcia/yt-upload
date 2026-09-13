mod auth;
mod chunk;
mod cli;
mod metadata;
mod resumable_upload;
mod state;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    let client_secret = auth::load_client_secret(&auth::client_secret_path()).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nCreate an OAuth 'Desktop app' client in Google Cloud Console and save its JSON to {}",
            auth::client_secret_path().display()
        )
    })?;

    let access_token =
        auth::get_valid_access_token(&client_secret, &auth::token_cache_path()).await?;

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

    Ok(())
}
