use crate::manifest::WorkspaceKind;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// A jj workspace stores the absolute or relative path to the *main* repo's
// .jj/repo directory inside <workspace>/.jj/repo (a regular file). A git
// worktree stores `gitdir: <abs-path>` in <worktree>/.git (also a regular
// file). In both cases the worktree's metadata references an absolute host
// path that won't exist inside the container unless we bind-mount the base
// repo at the same path. This returns that base path (the main repo's
// working tree, not its .jj/.git dir) or None if the project isn't a
// worktree/workspace.
pub fn detect_vcs_base(dir: &Path) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    if let Some(b) = detect_jj_base(dir) {
        return Some(b);
    }
    detect_git_base(dir)
}

fn detect_jj_base(dir: &Path) -> Option<PathBuf> {
    let repo_file = dir.join(".jj").join("repo");
    if !repo_file.is_file() {
        return None;
    }
    let raw = fs::read_to_string(&repo_file).ok()?;
    let raw = raw.trim_end_matches('\n');

    let target: PathBuf = if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        let candidate = dir.join(".jj").join(raw);
        std::fs::canonicalize(&candidate).ok()?
    };

    // target is <main>/.jj/repo; the working tree is two levels up.
    let target_str = target.to_string_lossy();
    let base = target_str.strip_suffix("/.jj/repo").unwrap_or(&target_str);
    if base.is_empty() || Path::new(base) == target || Path::new(base) == dir {
        return None;
    }
    Some(PathBuf::from(base))
}

fn detect_git_base(dir: &Path) -> Option<PathBuf> {
    let git_file = dir.join(".git");
    // Only handle the worktree case (.git is a regular file). A regular
    // checkout's .git is a directory; nothing to rewire.
    if !git_file.is_file() {
        return None;
    }
    let text = fs::read_to_string(&git_file).ok()?;
    let line = text.lines().find(|l| l.starts_with("gitdir:"))?;
    let mut gitdir = line.trim_start_matches("gitdir:").trim().to_string();
    if !gitdir.starts_with('/') {
        let candidate = dir.join(&gitdir);
        gitdir = std::fs::canonicalize(&candidate)
            .ok()?
            .to_string_lossy()
            .into_owned();
    }

    // gitdir is typically <main>/.git/worktrees/<name>, sometimes <main>/.git.
    let base: String = if let Some(idx) = gitdir.find("/.git/worktrees/") {
        gitdir[..idx].to_string()
    } else if let Some(stripped) = gitdir.strip_suffix("/.git") {
        stripped.to_string()
    } else {
        gitdir.clone()
    };
    if base.is_empty() || base == gitdir || Path::new(&base) == dir {
        return None;
    }
    Some(PathBuf::from(base))
}

/// The revision a new workspace branches from when `--from` is omitted: the
/// project's default branch.
///
/// jj: the built-in `trunk()` revset — it resolves `main@origin` /
/// `master@origin` / … itself, so the main-vs-master distinction needs no
/// special-casing here.
///
/// git: the branch `refs/remotes/origin/HEAD` points at, returned as the
/// remote-tracking ref (`origin/<branch>`) so the workspace starts from
/// upstream's tip rather than a possibly-stale local branch. That symref is
/// per-repo, so `main` vs `master` is whatever the repo was cloned with.
/// `git clone` writes it, but not every setup has it; when it's missing,
/// `origin/main` and `origin/master` are probed and a unique hit is used.
pub fn default_branch(kind: WorkspaceKind, repo: &Path) -> Result<String> {
    match kind {
        WorkspaceKind::Jj => Ok("trunk()".to_string()),
        WorkspaceKind::Git => git_default_branch(repo),
    }
}

fn git_default_branch(repo: &Path) -> Result<String> {
    // origin/HEAD records the remote's default branch as of clone time.
    if let Some(short) = git_capture(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )? {
        return Ok(short); // already in `origin/<branch>` form
    }
    // No symref — probe the usual names, and accept the result only when
    // exactly one exists so we never silently pick the wrong default.
    let found: Vec<&str> = ["main", "master"]
        .into_iter()
        .filter(|b| {
            let r = format!("refs/remotes/origin/{b}");
            git_ok(repo, &["show-ref", "--verify", "--quiet", &r])
        })
        .collect();
    match found.as_slice() {
        [b] => Ok(format!("origin/{b}")),
        _ => bail!(
            "could not determine the default branch of {} (no origin/HEAD). \
             Pass --from explicitly, or run `git remote set-head origin --auto`.",
            repo.display()
        ),
    }
}

// `git -C <repo> <args>`: trimmed stdout when it succeeds with output, else
// None (a failed command or empty output).
fn git_capture(repo: &Path, args: &[&str]) -> Result<Option<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .context("spawning git")?;
    if !out.status.success() {
        return Ok(None);
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(if s.is_empty() { None } else { Some(s) })
}

// `git -C <repo> <args>`: true when the command exits 0.
fn git_ok(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maudebox-vcs-{label}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn detects_absolute_git_worktree_base() {
        let root = temp_dir("git");
        let base = root.join("main");
        let worktree = root.join("worktree");
        fs::create_dir_all(base.join(".git/worktrees/feature")).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}/.git/worktrees/feature\n", base.display()),
        )
        .unwrap();
        assert_eq!(detect_vcs_base(&worktree), Some(base));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn regular_checkout_has_no_extra_base() {
        let root = temp_dir("regular");
        fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(detect_vcs_base(&root), None);
        fs::remove_dir_all(root).unwrap();
    }
}
