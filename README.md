# yt-upload

A small CLI that uploads a video to YouTube using the resumable upload
protocol.

## Setup

1. Create a Google Cloud project and enable the YouTube Data API v3.
2. Create an OAuth client ID of type **Desktop app**.
3. Download its JSON and save it to:
   - Linux: `~/.config/yt-upload/client_secret.json`
   - macOS: `~/Library/Application Support/yt-upload/client_secret.json`
   - Windows: `%APPDATA%\yt-upload\client_secret.json`

## Usage

```bash
yt-upload video.mp4 --title "My Video" \
  --description "A description" \
  --tags rust,youtube \
  --privacy unlisted \
  --category 22
```

On first run, a browser window opens for you to authorize the app; the
resulting token is cached locally so later runs don't need to reauthorize.

If the upload is interrupted (network drop, process killed), re-running the
same command on the same file resumes from the last confirmed byte instead
of starting over.
