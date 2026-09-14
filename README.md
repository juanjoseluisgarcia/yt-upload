<p align="center">
  <img src="assets/logo.svg" width="120" alt="yt-upload logo">
</p>

<h1 align="center">yt-upload</h1>

A small CLI that uploads a video to YouTube using the resumable upload
protocol.

> This project is not affiliated with, sponsored by, or endorsed by YouTube
> or Google. "YouTube" is a trademark of Google LLC.

## Install

```bash
brew tap juanjoseluisgarcia/yt-upload
brew install yt-upload
```

This builds from source (Rust must be available, which Homebrew installs
automatically as a build dependency) and also installs man pages —
`man yt-upload`, `man yt-upload-upload`, `man yt-upload-login`, etc.

## Setup

yt-upload doesn't ship a built-in Google API client — you authorize it
with your own, which is free and takes about five minutes:

1. Create a Google Cloud project and enable the YouTube Data API v3.
2. Create an OAuth client ID of type **Desktop app**.
3. Download its JSON and save it to:
   - Linux: `~/.config/yt-upload/client_secret.json`
   - macOS: `~/Library/Application Support/yt-upload/client_secret.json`
   - Windows: `%APPDATA%\yt-upload\client_secret.json`

For the full walkthrough — exact console menus, the consent-screen
"Testing" mode caveat (7-day token expiry) and how to avoid it — run
`yt-upload login --help` or see `man yt-upload-login`.

## Usage

Authorize the CLI once:

```bash
yt-upload login
```

This opens a browser window for you to authorize the app; the resulting
token is cached locally so later commands don't need to reauthorize. Run
`yt-upload login --force` to switch accounts or re-authorize explicitly.

Check whether you're currently authorized, and as which account:

```bash
yt-upload status
```

Forget the cached credentials:

```bash
yt-upload logout
```

Upload a video:

```bash
yt-upload upload video.mp4 --title "My Video" \
  --description "A description" \
  --tags rust,youtube \
  --privacy unlisted \
  --category 22
```

`upload` requires an existing session — if none exists it errors out with
a pointer to run `yt-upload login`, rather than opening a browser
mid-upload.

If the upload is interrupted (network drop, process killed), re-running
the same command on the same file resumes from the last confirmed byte
instead of starting over.

Run `yt-upload --help` (or `yt-upload <command> --help`) for the full list
of flags for any command.

## License

MIT — see [LICENSE](LICENSE).
