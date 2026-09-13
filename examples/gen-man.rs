//! Renders yt-upload's man pages (the top-level command plus one per
//! subcommand) into `man/`, from the same `Cli` definition clap uses to
//! parse arguments, so the pages can never drift from actual behavior.
//! Run via `make man`; never installed as a binary.

use clap::CommandFactory;
use std::path::Path;

include!("../src/cli.rs");

const VERSION: &str = env!("CARGO_PKG_VERSION");
// Update this when regenerating man pages for a new release (`make man`).
const DATE: &str = "2026-09-13";

/// Renders `cmd` to `<out_dir>/<name>.1`, then recurses into every
/// subcommand. `Cli` disables clap's auto-generated `help` pseudo-command
/// (see cli.rs), so every subcommand here is a real one worth its own page
/// — and every page this generates is one the top-level page's SUBCOMMANDS
/// section actually links to, with no dangling cross-references.
fn render_recursive(cmd: &clap::Command, out_dir: &Path) -> std::io::Result<()> {
    let name = cmd
        .get_display_name()
        .unwrap_or_else(|| cmd.get_name())
        .to_owned();
    let man = clap_mangen::Man::new(cmd.clone())
        .title(name.to_uppercase())
        .date(DATE)
        .source(format!("yt-upload {VERSION}"))
        .manual("General Commands Manual");
    let mut buffer = Vec::new();
    man.render(&mut buffer)?;
    std::fs::write(out_dir.join(format!("{name}.1")), buffer)?;

    for sub in cmd.get_subcommands() {
        render_recursive(sub, out_dir)?;
    }
    Ok(())
}

fn main() {
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("man");
    std::fs::create_dir_all(&out_dir).expect("failed to create man/ directory");

    let mut cmd = Cli::command();
    cmd.build();
    render_recursive(&cmd, &out_dir).expect("failed to render man pages");
}
