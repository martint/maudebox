use crate::paths::xdg_state_home;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

// Bash `echo "$x" | sha256sum` includes the trailing newline echo adds —
// matching it byte-for-byte is what keeps the rust port's volume names
// compatible with volumes created by the bash version.
pub fn sha256_prefix(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher.update(b"\n");
    let digest = hasher.finalize();
    hex::encode(&digest[..4])
}

// Project key: human-readable basename + 8-char hash of the full path. Used
// as the prefix for every overlay volume on a given project and as the
// state-dir name.
pub fn compute_volume_name(project_dir: &Path) -> String {
    let basename = project_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let s = project_dir.to_string_lossy();
    format!("maudebox-overlay-{basename}-{}", sha256_prefix(&s))
}

// Per-overlay volume name: project key + 8-char hash of the container target.
pub fn compute_overlay_volume(project_dir: &Path, container_target: &str) -> String {
    format!(
        "{}-{}",
        compute_volume_name(project_dir),
        sha256_prefix(container_target)
    )
}

// Per-instance state dir on the host. Bind-mounted at /run/maudebox inside
// the container for ephemeral instances so `maudebox-keep` (or host-side
// `maudebox keep`) can drop a flag the cleanup trap reads after exit.
pub fn compute_state_dir(project_dir: &Path) -> Result<PathBuf> {
    Ok(xdg_state_home()?
        .join("maudebox")
        .join("instances")
        .join(compute_volume_name(project_dir)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These values are what `echo "$x" | sha256sum | cut -c1-8` produces in
    // bash; if this assertion ever breaks, existing volumes created by the
    // bash version stop being recognized.
    #[test]
    fn volume_names_match_bash() {
        let p = Path::new("/home/martin/projects/myproject");
        assert_eq!(
            compute_volume_name(p),
            "maudebox-overlay-myproject-6b166a34"
        );
        assert_eq!(
            compute_overlay_volume(p, "/root/.m2"),
            "maudebox-overlay-myproject-6b166a34-fee00b7a"
        );
    }

    #[test]
    fn sha256_includes_trailing_newline() {
        // Without the trailing newline this would be "9d10aef5"; with it,
        // "6b166a34". The bash version uses `echo` which appends a newline,
        // so we have to match that for volume-name compatibility.
        assert_eq!(sha256_prefix("/home/martin/projects/myproject"), "6b166a34");
    }
}
