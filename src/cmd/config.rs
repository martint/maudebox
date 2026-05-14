use crate::paths::config_path;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::fs;
use std::process::Command;

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Open the user config in $VISUAL / $EDITOR (falls back to `vi`).
    Edit,
    /// Print the path to the user config file.
    Path,
}

pub fn run(action: ConfigAction) -> Result<i32> {
    let path = config_path()?;
    match action {
        ConfigAction::Path => {
            println!("{}", path.display());
            Ok(0)
        }
        ConfigAction::Edit => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let editor = std::env::var("VISUAL")
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.is_empty()))
                .unwrap_or_else(|| "vi".to_string());
            // Run the editor through `sh -c` so values like `code --wait` (with
            // embedded flags) work as-is. The script is `<editor> "$@"`; the
            // config path is appended as the only positional, so it survives
            // any quoting the user already baked into $EDITOR.
            let script = format!("{editor} \"$@\"");
            let status = Command::new("sh")
                .arg("-c")
                .arg(&script)
                .arg("sh")
                .arg(path.as_os_str())
                .status()
                .with_context(|| format!("spawning editor ({editor})"))?;
            Ok(status.code().unwrap_or(1))
        }
    }
}
