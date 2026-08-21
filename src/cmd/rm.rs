use crate::docker;
use crate::manifest::{self, Manifest, WorkspaceKind};
use crate::paths::{canonicalize, looks_like_path};
use crate::volume::compute_state_dir;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Remove a maudebox *project*: every volume labelled with it, its state dir,
// and — if the state dir's manifest says maudebox created the worktree — the
// workspace/worktree itself. For a path that maudebox didn't create (i.e. a
// project launched via `maudebox <path>`), the worktree is left alone; we only
// clean up what we put there.
//
// All of that is project-scoped, and saying so matters: an instance handle used
// to be accepted as though it named something removable, so `maudebox rm
// trino-review` destroyed state shared with every other instance of that
// project — and, for a `maudebox new` workspace, the tree they were all working
// in. A handle that names a *running* container still resolves, because that is
// a natural way to point at a live thing, but it resolves to that container's
// project and removal is refused while any container is up.
//
// The identifier accepted is:
//   - a path (absolute, relative, or `~`-prefixed)
//   - a project basename (the project dir's last component)
//   - a running container's ID, instance handle, or ephemeral-name
pub fn run(target: &str) -> Result<i32> {
    let project_dir = resolve_target(target)?;
    let state_dir = compute_state_dir(&project_dir)?;

    // Pulling volumes out from under a live container corrupts whatever it is
    // doing; the state dir also backs the ephemeral keep flag it may still
    // write. Name the instances so the fix is obvious.
    let live = running_instances(&project_dir)?;
    if !live.is_empty() {
        bail!(
            "{} has {} running: {}\nStop {} before removing the project.",
            project_dir.display(),
            if live.len() == 1 {
                "an instance"
            } else {
                "instances"
            },
            live.join(", "),
            if live.len() == 1 { "it" } else { "them" },
        );
    }

    // Manifest present → maudebox created the worktree; tear it down.
    let manifest_opt = manifest::read(&state_dir)?;
    if let Some(m) = &manifest_opt {
        tear_down_workspace(m, &project_dir)?;
    }

    // Project-scoped volumes (overlays and the Codex daemon runtime) are
    // matched by label rather than name suffix.
    let label = format!("label=maudebox.project={}", project_dir.display());
    let out = docker::capture(&["volume", "ls", "--filter", &label, "--format", "{{.Name}}"])
        .unwrap_or_default();
    let volumes: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    for v in &volumes {
        docker::capture(&["volume", "rm", v])?;
        println!("Removed volume: {v}");
    }

    let state_existed = state_dir.exists();
    if state_existed {
        fs::remove_dir_all(&state_dir)
            .with_context(|| format!("removing {}", state_dir.display()))?;
        println!("Removed state dir: {}", state_dir.display());
    }

    if manifest_opt.is_none() && volumes.is_empty() && !state_existed {
        println!("Nothing to remove for: {}", project_dir.display());
    }
    Ok(0)
}

// Map a user-supplied identifier (path, project basename, or a running
// container's id / instance handle / ephemeral-name) to a canonical project
// path. Path-shaped inputs short-circuit; otherwise we consult docker volume
// and container labels.
fn resolve_target(target: &str) -> Result<PathBuf> {
    if looks_like_path(target) {
        return Ok(canonicalize(target));
    }
    let from_volumes = lookup_by_volume_label(target).unwrap_or_default();
    let from_containers = lookup_by_container_label(target).unwrap_or_default();
    let mut all: Vec<String> = from_volumes;
    for p in from_containers {
        if !all.contains(&p) {
            all.push(p);
        }
    }
    if all.is_empty() {
        // Fall back to treating it as a path — covers orphans whose docker
        // state was wiped out from under us.
        return Ok(canonicalize(target));
    }
    if all.len() > 1 {
        let mut msg = format!("Multiple instances match '{target}':\n");
        for p in &all {
            msg.push_str(&format!("  {p}\n"));
        }
        msg.push_str("Re-run with the full path.");
        bail!("{msg}");
    }
    Ok(PathBuf::from(&all[0]))
}

// Match a bare name against the *basename* of each project that still owns a
// volume. Matching the volume's `maudebox.instance` label instead would resolve
// a handle like `trino-review` to a project whose state it does not own, which
// is the confusion this command is being cured of.
fn lookup_by_volume_label(name: &str) -> Result<Vec<String>> {
    let out = docker::capture(&[
        "volume",
        "ls",
        "--filter",
        "label=maudebox.project",
        "--format",
        "{{.Label \"maudebox.project\"}}",
    ])?;
    Ok(parse_volume_matches(&out, name))
}

fn parse_volume_matches(out: &str, name: &str) -> Vec<String> {
    let mut out_paths = Vec::new();
    for project in out.lines() {
        let basename = Path::new(project)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if basename == name && !project.is_empty() && !out_paths.contains(&project.to_string()) {
            out_paths.push(project.to_string());
        }
    }
    out_paths
}

// Instance handles of containers currently running against a project.
fn running_instances(project_dir: &Path) -> Result<Vec<String>> {
    let filter = format!("label=maudebox.project={}", project_dir.display());
    let out = docker::capture(&[
        "ps",
        "--filter",
        &filter,
        "--format",
        "{{.Label \"maudebox.instance\"}}",
    ])
    .unwrap_or_default();
    let mut names: Vec<String> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    names.sort();
    Ok(names)
}

fn lookup_by_container_label(name: &str) -> Result<Vec<String>> {
    let fmt = "{{.ID}}\t{{.Label \"maudebox.instance\"}}\t{{.Label \"maudebox.ephemeral-name\"}}\t{{.Label \"maudebox.project\"}}";
    let out = docker::capture(&["ps", "--filter", "label=maudebox.instance", "--format", fmt])?;
    Ok(parse_container_matches(&out, name))
}

fn parse_container_matches(out: &str, name: &str) -> Vec<String> {
    let mut out_paths = Vec::new();
    for line in out.lines() {
        let mut it = line.split('\t');
        let id = it.next().unwrap_or("");
        let instance = it.next().unwrap_or("");
        let ename = it.next().unwrap_or("");
        let project = it.next().unwrap_or("");
        if (id == name || instance == name || ename == name)
            && !project.is_empty()
            && !out_paths.contains(&project.to_string())
        {
            out_paths.push(project.to_string());
        }
    }
    out_paths
}

fn tear_down_workspace(m: &Manifest, target: &Path) -> Result<()> {
    let source = Path::new(&m.source);
    let kind = m.kind;
    println!(
        "Removing {} workspace '{}' at: {}",
        kind.name(),
        m.name,
        target.display()
    );
    match kind {
        WorkspaceKind::Jj => {
            run_in(source, "jj", &["workspace", "forget", &m.name])?;
            fs::remove_dir_all(target).with_context(|| format!("removing {}", target.display()))?;
        }
        WorkspaceKind::Git => {
            let target_str = target.display().to_string();
            run_in(
                source,
                "git",
                &["worktree", "remove", "--force", &target_str],
            )?;
            run_in(source, "git", &["branch", "-D", &m.name])?;
        }
    }
    Ok(())
}

fn run_in(cwd: &Path, cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("spawning {cmd}"))?;
    if !status.success() {
        bail!("{cmd} {} failed (exit {})", args.join(" "), status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_selection_matches_project_basename_and_deduplicates() {
        let input = "/src/a\n/src/a\n/src/b\n";
        assert_eq!(parse_volume_matches(input, "a"), vec!["/src/a"]);
    }

    // `trino-review` is an instance handle, not a project. Resolving it through
    // volumes would point `rm` at /src/trino and destroy state shared with every
    // other instance; only a live container may answer for a handle.
    #[test]
    fn volume_selection_ignores_instance_handles() {
        assert!(parse_volume_matches("/src/trino\n", "trino-review").is_empty());
    }

    #[test]
    fn container_selection_accepts_id_instance_or_ephemeral_name() {
        let input = "abc123\trepo\tfeature\t/src/a\ndef456\tother\tscratch\t/src/b\n";
        assert_eq!(parse_container_matches(input, "abc123"), vec!["/src/a"]);
        assert_eq!(parse_container_matches(input, "repo"), vec!["/src/a"]);
        assert_eq!(parse_container_matches(input, "scratch"), vec!["/src/b"]);
    }
}
