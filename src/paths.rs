use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const CONTAINER_HOME: &str = "/root";
pub const IMAGE_NAME_DEFAULT: &str = "maudebox";
pub const STATE_VOLUME: &str = "maudebox-state";

pub fn home() -> Result<PathBuf> {
    let h = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(h))
}

pub fn xdg_config_home() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("XDG_CONFIG_HOME") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    Ok(home()?.join(".config"))
}

pub fn xdg_state_home() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("XDG_STATE_HOME") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    Ok(home()?.join(".local").join("state"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(xdg_config_home()?.join("maudebox").join("config.toml"))
}

// Bash `realpath` dereferences symlinks AND tolerates missing paths on most
// distros; std::fs::canonicalize dereferences but errors on missing. This
// matches the bash script's effective behavior so volume-name hashes stay
// consistent for symlinked project paths.
pub fn canonicalize<P: AsRef<Path>>(p: P) -> PathBuf {
    let p = p.as_ref();
    match std::fs::canonicalize(p) {
        Ok(c) => c,
        Err(_) => {
            // Fall back: absolutize without symlink resolution.
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(p))
                    .unwrap_or_else(|_| p.to_path_buf())
            }
        }
    }
}

// Expand a leading `~` on the host side of a mount spec.
pub fn expand_host_tilde(p: &str) -> Result<String> {
    if let Some(rest) = p.strip_prefix('~') {
        let h = home()?;
        Ok(format!("{}{}", h.display(), rest))
    } else {
        Ok(p.to_string())
    }
}

// Expand a leading `~` on the container side of a mount spec.
pub fn expand_container_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix('~') {
        format!("{}{}", CONTAINER_HOME, rest)
    } else {
        p.to_string()
    }
}
