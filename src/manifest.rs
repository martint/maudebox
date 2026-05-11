// Per-instance manifest written into the state dir when `maudebox new` creates
// a workspace/worktree. Its presence is the marker that maudebox itself owns
// the worktree at the project path, so `rm` knows to tear it down (vs. a
// path the user handed to `maudebox <path>` directly, which maudebox didn't
// create and must leave alone).

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
        };
        write(&tmp, &m).unwrap();
        let back = read(&tmp).unwrap().unwrap();
        assert_eq!(back.kind, WorkspaceKind::Jj);
        assert_eq!(back.source, m.source);
        assert_eq!(back.name, m.name);
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
