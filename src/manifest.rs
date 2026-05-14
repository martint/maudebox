// Per-instance manifest written into the state dir when `maudebox new` creates
// a workspace/worktree. Its presence is the marker that maudebox itself owns
// the worktree at the project path, so `rm` knows to tear it down (vs. a
// path the user handed to `maudebox <path>` directly, which maudebox didn't
// create and must leave alone).

use crate::paths::xdg_state_home;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceKind {
    Jj,
    Git,
}

impl WorkspaceKind {
    pub fn name(self) -> &'static str {
        match self {
            WorkspaceKind::Jj => "jj",
            WorkspaceKind::Git => "git",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub kind: WorkspaceKind,
    /// Path to the parent repo from which the workspace/worktree was created.
    pub source: String,
    /// Workspace/branch name as passed to `maudebox new`.
    pub name: String,
    /// Host path of the workspace/worktree itself. Used by `maudebox <name>`
    /// to reattach without typing the path. Defaulted for backward
    /// compatibility with manifests written before this field existed.
    #[serde(default)]
    pub target: String,
}

pub fn manifest_path(state_dir: &Path) -> PathBuf {
    state_dir.join("manifest.toml")
}

pub fn write(state_dir: &Path, manifest: &Manifest) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))?;
    let path = manifest_path(state_dir);
    let body = toml::to_string_pretty(manifest).context("serializing manifest")?;
    fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn read(state_dir: &Path) -> Result<Option<Manifest>> {
    let path = manifest_path(state_dir);
    match fs::read_to_string(&path) {
        Ok(s) => {
            let m: Manifest = toml::from_str(&s)
                .with_context(|| format!("parsing {}", path.display()))?;
            Ok(Some(m))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

// Walk $XDG_STATE_HOME/maudebox/instances/* and return every manifest whose
// `name` matches `query` and whose recorded target still exists on disk.
// Used by `maudebox <name>` to reattach without the user typing the worktree
// path. Manifests written before the `target` field existed are skipped (the
// field is empty, so there's nothing to reattach to); the user can fall back
// to `maudebox <path>`.
pub fn find_by_name(query: &str) -> Result<Vec<(PathBuf, Manifest)>> {
    let root = xdg_state_home()?.join("maudebox").join("instances");
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", root.display())),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(Some(m)) = read(&path) else { continue };
        if m.name != query || m.target.is_empty() {
            continue;
        }
        let target = PathBuf::from(&m.target);
        if !target.is_dir() {
            continue;
        }
        out.push((target, m));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn manifest_round_trip() {
        let tmp = env::temp_dir().join(format!("maudebox-manifest-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let m = Manifest {
            kind: WorkspaceKind::Jj,
            source: "/home/user/projects/myproj".into(),
            name: "feature-x".into(),
            target: "/home/user/projects/myproj.feature-x".into(),
        };
        write(&tmp, &m).unwrap();
        let back = read(&tmp).unwrap().unwrap();
        assert_eq!(back.kind, WorkspaceKind::Jj);
        assert_eq!(back.source, m.source);
        assert_eq!(back.name, m.name);
        assert_eq!(back.target, m.target);
        fs::remove_dir_all(&tmp).unwrap();
    }

    // Pre-target-field manifests should still load; the field defaults to "".
    #[test]
    fn manifest_legacy_no_target_field() {
        let tmp = env::temp_dir()
            .join(format!("maudebox-manifest-legacy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let legacy = r#"kind = "git"
source = "/home/user/projects/myproj"
name = "feature-x"
"#;
        fs::write(manifest_path(&tmp), legacy).unwrap();
        let back = read(&tmp).unwrap().unwrap();
        assert_eq!(back.kind, WorkspaceKind::Git);
        assert_eq!(back.name, "feature-x");
        assert_eq!(back.target, "");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn manifest_missing_is_none() {
        let tmp = env::temp_dir().join(format!("maudebox-manifest-absent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        assert!(read(&tmp).unwrap().is_none());
        fs::remove_dir_all(&tmp).unwrap();
    }
}
