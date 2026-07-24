use crate::docker;
use crate::manifest::{self, Manifest, WorkspaceKind};
use crate::paths::{canonicalize, looks_like_path};
use crate::volume::compute_state_dir;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Remove a maudebox-managed instance: its project volumes, its state dir,
// and — if the state dir's manifest says maudebox created the worktree —
// the workspace/worktree itself. For a path that maudebox didn't create
// (i.e. a project launched via `maudebox <path>`), the worktree is left
// alone; we only clean up what we put there.
//
// The identifier accepted is the same shape as `keep`:
//   - container ID
//   - instance basename (the project dir's last component)
//   - ephemeral-name (the `<name>` from `maudebox new <name>`)
//   - or a path (absolute, relative, or `~`-prefixed)
pub fn run(target: &str) -> Result<i32> {
    let project_dir = resolve_target(target)?;
    let state_dir = compute_state_dir(&project_dir)?;

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

// Map a user-supplied identifier (id, instance basename, ephemeral-name, or
// path) to a canonical project path. Path-shaped inputs short-circuit;
// otherwise we consult docker volume + container labels.
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

fn lookup_by_volume_label(name: &str) -> Result<Vec<String>> {
    let fmt = "{{.Label \"maudebox.instance\"}}\t{{.Label \"maudebox.project\"}}";
    let out = docker::capture(&[
        "volume",
        "ls",
        "--filter",
        "name=maudebox-overlay-",
        "--format",
        fmt,
    ])?;
    Ok(parse_volume_matches(&out, name))
}

fn parse_volume_matches(out: &str, name: &str) -> Vec<String> {
    let mut out_paths = Vec::new();
    for line in out.lines() {
        let mut it = line.split('\t');
        let instance = it.next().unwrap_or("");
        let project = it.next().unwrap_or("");
        if instance == name && !project.is_empty() && !out_paths.contains(&project.to_string()) {
            out_paths.push(project.to_string());
        }
    }
    out_paths
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
    fn volume_selection_matches_instance_and_deduplicates_paths() {
        let input = "one\t/src/a\none\t/src/a\ntwo\t/src/b\n";
        assert_eq!(parse_volume_matches(input, "one"), vec!["/src/a"]);
    }

    #[test]
    fn container_selection_accepts_id_instance_or_ephemeral_name() {
        let input = "abc123\trepo\tfeature\t/src/a\ndef456\tother\tscratch\t/src/b\n";
        assert_eq!(parse_container_matches(input, "abc123"), vec!["/src/a"]);
        assert_eq!(parse_container_matches(input, "repo"), vec!["/src/a"]);
        assert_eq!(parse_container_matches(input, "scratch"), vec!["/src/b"]);
    }
}
