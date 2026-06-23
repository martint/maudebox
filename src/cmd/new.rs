use crate::cmd::run::{run as run_cmd, RunOptions};
use crate::cmd::rm;
use crate::manifest::{self, Manifest, WorkspaceKind};
use crate::resolve::resolve_project;
use crate::vcs;
use crate::volume::compute_state_dir;
use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args)]
pub struct NewArgs {
    /// New workspace/worktree name.
    pub name: String,

    /// Source project to branch from: a path, or a bare name resolved the
    /// same way as `maudebox <name>` — through the manifest store and the
    /// configured `project-roots` (default: current directory).
    #[arg(long, value_name = "PATH-OR-NAME")]
    pub source: Option<String>,

    /// Target path (default: <source>/../<basename>.<name>).
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,

    /// Starting revision (default: the project's default branch — jj
    /// `trunk()`, git `origin/HEAD`). Pass `--from @-` (jj) or `--from HEAD`
    /// (git) to branch from the source's current checkout instead.
    #[arg(long, value_name = "REV")]
    pub from: Option<String>,

    /// Refresh the source repo's remote-tracking refs (`git fetch` /
    /// `jj git fetch`) before resolving the default branch.
    #[arg(long)]
    pub fetch: bool,

    /// Tear down workspace/worktree and overlay volume on container exit.
    #[arg(long)]
    pub ephemeral: bool,

    /// Command to run inside the new workspace (default: interactive shell).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

// Create a jj workspace or git worktree from a source project and run
// maudebox on it. By default the workspace persists after the container
// exits, like `maudebox <path>` does. With --ephemeral, the workspace and
// its overlay volume are torn down on exit. jj is preferred when both .jj
// and .git are present (colocated repos).
pub fn run(
    args: NewArgs,
    image: String,
    extra_mounts: Vec<String>,
    instance: Option<String>,
    network: Vec<String>,
) -> Result<i32> {
    let NewArgs {
        name,
        source: source_opt,
        path: target_opt,
        from: from_opt,
        fetch,
        ephemeral,
        command: inner_cmd,
    } = args;

    // The source project is named explicitly via --source (a path or a bare
    // name resolved like `maudebox <name>`); everything after <name> is the
    // container command, with no positional source to disambiguate from it.
    let source_str = match source_opt.as_deref() {
        None | Some("") => ".".to_string(),
        Some(s) => resolve_project(s)?,
    };
    let source = std::fs::canonicalize(&source_str)
        .with_context(|| format!("resolving source: {source_str}"))?;
    if !source.is_dir() {
        eprintln!("Not a directory: {}", source.display());
        return Ok(1);
    }

    let target: PathBuf = match target_opt.as_deref() {
        None | Some("") => {
            let parent = source.parent().unwrap_or(Path::new("/"));
            let basename = source
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            parent.join(format!("{basename}.{name}"))
        }
        Some(t) if Path::new(t).is_absolute() => PathBuf::from(t),
        Some(t) => std::env::current_dir()?.join(t),
    };

    if target.exists() {
        eprintln!("Target path already exists: {}", target.display());
        return Ok(1);
    }

    // VCS kind drives both the optional fetch and the default-branch
    // resolution below. jj wins for colocated repos (.jj + .git present).
    let kind = if source.join(".jj").exists() {
        WorkspaceKind::Jj
    } else if source.join(".git").exists() {
        WorkspaceKind::Git
    } else {
        eprintln!("Not a jj or git repo: {}", source.display());
        return Ok(1);
    };

    // --fetch refreshes the source repo's remote-tracking refs first, so the
    // workspace starts from current upstream rather than the last fetch.
    if fetch {
        match kind {
            WorkspaceKind::Jj => run_in(&source, "jj", &["git".into(), "fetch".into()])?,
            WorkspaceKind::Git => run_in(
                &source,
                "git",
                &["fetch".into(), "--quiet".into(), "origin".into()],
            )?,
        }
    }

    // Starting revision: an explicit --from, else the project's default
    // branch (jj `trunk()` / git `origin/HEAD`).
    let from_rev = match from_opt {
        Some(r) if !r.is_empty() => r,
        _ => vcs::default_branch(kind, &source)?,
    };

    match kind {
        WorkspaceKind::Jj => {
            println!("Creating jj workspace '{name}' at: {}", target.display());
            run_in(
                &source,
                "jj",
                &[
                    "workspace".into(),
                    "add".into(),
                    "--name".into(),
                    name.clone(),
                    "-r".into(),
                    from_rev.clone(),
                    target.display().to_string(),
                ],
            )?;
        }
        WorkspaceKind::Git => {
            println!(
                "Creating git worktree '{name}' at: {} (branch: {name} from {from_rev})",
                target.display()
            );
            run_in(
                &source,
                "git",
                &[
                    "worktree".into(),
                    "add".into(),
                    "-b".into(),
                    name.clone(),
                    target.display().to_string(),
                    from_rev.clone(),
                ],
            )?;
        }
    }

    // Persist the manifest in the state dir. Its presence is what lets `rm`
    // distinguish a maudebox-created worktree from a user-handed path; the
    // recorded `target` is what lets `maudebox <name>` reattach without the
    // user typing the worktree path back.
    let discriminator = instance.unwrap_or_default();
    let state_dir = compute_state_dir(&target)?;
    manifest::write(
        &state_dir,
        &Manifest {
            kind,
            source: source.display().to_string(),
            name: name.clone(),
            target: target.display().to_string(),
            instance: discriminator.clone(),
        },
    )?;

    // Recursive launch — call run_cmd directly rather than spawning another
    // wrapper process. Same effect, fewer hops.
    let rc = run_cmd(RunOptions {
        image,
        memory_from: source.display().to_string(),
        extra_mounts,
        instance: discriminator,
        ephemeral_name: if ephemeral { name.clone() } else { String::new() },
        network,
        project_dir: target.display().to_string(),
        command: inner_cmd,
    });

    if ephemeral {
        if let Err(e) = ephemeral_cleanup(&target) {
            eprintln!("cleanup error: {e}");
        }
    }

    rc
}

// Run after an `--ephemeral` container exits. The user may have disarmed
// cleanup mid-session by dropping a `keep` file (via in-container
// `maudebox-keep` or host `maudebox keep`); honour that by removing only the
// keep flag and leaving the manifest in place, so the workspace is preserved
// AND a future `rm` still recognizes it as maudebox-owned.
fn ephemeral_cleanup(target: &Path) -> Result<()> {
    let state_dir = compute_state_dir(target)?;
    let keep = state_dir.join("keep");
    if keep.exists() {
        println!("Keep flag set; preserving workspace at: {}", target.display());
        let _ = std::fs::remove_file(&keep);
        return Ok(());
    }
    // Full teardown lives in `rm` now — manifest-driven, identifier-aware,
    // and shared with the host-side `maudebox rm` command.
    let _ = rm::run(&target.display().to_string());
    Ok(())
}

fn run_in(cwd: &Path, cmd: &str, args: &[String]) -> Result<()> {
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
