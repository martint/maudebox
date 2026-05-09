use std::fs;
use std::path::{Path, PathBuf};

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
    let base = target_str
        .strip_suffix("/.jj/repo")
        .unwrap_or(&target_str);
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
