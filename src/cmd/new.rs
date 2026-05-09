use crate::cmd::run::{run as run_cmd, RunOptions};
use crate::docker;
use crate::volume::{compute_state_dir, compute_volume_name};
use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args)]
pub struct NewArgs {
    /// New workspace/worktree name.
    pub name: String,

    /// Target path (default: <source>/../<basename>.<name>).
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,

    /// Starting revision (default: jj @-, git HEAD).
    #[arg(long, value_name = "REV")]
    pub from: Option<String>,

    /// Tear down workspace/worktree and overlay volume on container exit.
    #[arg(long)]
    pub ephemeral: bool,

    /// [source-dir] [command...] — source-dir defaults to cwd; if the first
    /// positional after `name` is a directory it's treated as the source,
    /// otherwise everything is the container command.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "REST")]
    pub rest: Vec<String>,
}

#[derive(Clone, Copy)]
enum Kind {
    Jj,
    Git,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Jj => "jj",
            Kind::Git => "git",
        }
    }
}

// Create a jj workspace or git worktree from a source project and run
// maudebox on it. By default the workspace persists after the container
// exits, like `maudebox <path>` does. With --ephemeral, the workspace and
// its overlay volume are torn down on exit. jj is preferred when both .jj
// and .git are present (colocated repos).
pub fn run(args: NewArgs, image: String, extra_mounts: Vec<String>) -> Result<i32> {
    let NewArgs {
        name,
        path: target_opt,
        from: from_opt,
        ephemeral,
        rest,
    } = args;
    let from_rev = from_opt.unwrap_or_default();

    // Positional shape after <name>:
    //   <source-dir> <command...>   if rest[0] exists as a directory
    //   <command...>                otherwise (source defaults to cwd)
    let (source_str, inner_cmd): (String, Vec<String>) = match rest.first() {
        Some(first) if Path::new(first).is_dir() => (first.clone(), rest[1..].to_vec()),
        Some(_) => (".".to_string(), rest),
        None => (".".to_string(), Vec::new()),
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

    let kind: Kind;
    if source.join(".jj").exists() {
        kind = Kind::Jj;
        println!("Creating jj workspace '{name}' at: {}", target.display());
        let mut jj_args: Vec<String> =
            vec!["workspace".into(), "add".into(), "--name".into(), name.clone()];
        if !from_rev.is_empty() {
            jj_args.push("-r".into());
            jj_args.push(from_rev.clone());
        }
        jj_args.push(target.display().to_string());
        run_in(&source, "jj", &jj_args)?;
    } else if source.join(".git").exists() {
        kind = Kind::Git;
        let rev: &str = if from_rev.is_empty() { "HEAD" } else { from_rev.as_str() };
        println!(
            "Creating git worktree '{name}' at: {} (branch: {name} from {rev})",
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
                rev.to_string(),
            ],
        )?;
    } else {
        eprintln!("Not a jj or git repo: {}", source.display());
        return Ok(1);
    }

    // Recursive launch — call run_cmd directly rather than spawning another
    // wrapper process. Same effect, fewer hops.
    let rc = run_cmd(RunOptions {
        image,
        memory_from: source.display().to_string(),
        extra_mounts,
        ephemeral_name: if ephemeral { name.clone() } else { String::new() },
        project_dir: target.display().to_string(),
        command: inner_cmd,
    });

    if ephemeral {
        if let Err(e) = ephemeral_cleanup(&name, &target, &source, kind) {
            eprintln!("cleanup error: {e}");
        }
    }

    rc
}

fn ephemeral_cleanup(name: &str, target: &Path, source: &Path, kind: Kind) -> Result<()> {
    let state_dir = compute_state_dir(target)?;

    // The user (in-container `maudebox-keep` or host `maudebox keep`) can
    // drop a `keep` file in the state dir to disarm cleanup mid-session.
    // Honour it — but still drop the now-stale state dir so a future
    // `maudebox` invocation against this same path starts fresh (it's no
    // longer ephemeral after this point).
    if state_dir.join("keep").exists() {
        println!(
            "Keep flag set; preserving {} workspace at: {}",
            kind.name(),
            target.display()
        );
        let _ = std::fs::remove_dir_all(&state_dir);
        return Ok(());
    }

    println!(
        "Cleaning up ephemeral {} workspace at: {}",
        kind.name(),
        target.display()
    );
    match kind {
        Kind::Jj => {
            let _ = run_in(source, "jj", &["workspace".into(), "forget".into(), name.into()]);
            let _ = std::fs::remove_dir_all(target);
        }
        Kind::Git => {
            let _ = run_in(
                source,
                "git",
                &[
                    "worktree".into(),
                    "remove".into(),
                    "--force".into(),
                    target.display().to_string(),
                ],
            );
            let _ = run_in(source, "git", &["branch".into(), "-D".into(), name.into()]);
        }
    }
    let volume = compute_volume_name(target);
    // best-effort: a non-overlay run never created the volume.
    let _ = docker::capture(&["volume", "rm", &volume]);
    let _ = std::fs::remove_dir_all(&state_dir);
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
