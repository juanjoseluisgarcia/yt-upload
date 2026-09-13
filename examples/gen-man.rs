//! Renders yt-upload's man pages (the top-level command plus one per
//! subcommand) into `man/`, from the same `Cli` definition clap uses to
//! parse arguments, so the pages can never drift from actual behavior.
//! Run via `make man`; never installed as a binary.

use clap::CommandFactory;
use std::path::Path;

include!("../src/cli.rs");

fn render_recursive(cmd: &clap::Command, out_dir: &Path) -> std::io::Result<()> {
    let man = clap_mangen::Man::new(cmd.clone());
    let mut buffer = Vec::new();
    man.render(&mut buffer)?;
    let name = cmd.get_display_name().unwrap_or_else(|| cmd.get_name());
    std::fs::write(out_dir.join(format!("{name}.1")), buffer)?;

    for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
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
