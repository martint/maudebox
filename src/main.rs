// maudebox – launch a maudebox container for a given worktree.
//
// Argument parsing uses clap (derive). The default-run shape — `maudebox
// [project] [command...]` — is captured via `external_subcommand`: anything
// that isn't a known subcommand becomes a Vec<String> we forward intact
// (including hyphen-prefixed args destined for the container command).

mod cmd;
mod config;
mod docker;
mod manifest;
mod mount;
mod paths;
mod resolve;
mod tmux;
mod vcs;
mod volume;

use anyhow::Result;
use clap::{Parser, Subcommand};
use paths::IMAGE_NAME_DEFAULT;
use std::process::ExitCode;

const LONG_ABOUT: &str = "\
Launch a maudebox container for a host project directory. The project (and \
its base repo, if it's a jj workspace or git worktree) is bind-mounted at \
its host path inside the container so VCS metadata still resolves. Extra \
host paths can be exposed via --mount or the user config; any of them can \
use mode `overlay` to layer a per-worktree writable upper over a read-only \
host lower (typically used for ~/.m2 to isolate Maven snapshots per \
worktree without mutating the host cache).";

const EXAMPLES: &str = "Examples:
  maudebox                               # interactive shell in current dir
  maudebox /path/to/myproject            # interactive shell, specific project
  maudebox . mvnd verify                 # run a build
  maudebox . claude                      # start Claude Code
  maudebox --instance review . claude    # second session on same repo (distinct $MAUDEBOX_INSTANCE)
  maudebox new feature-x                 # new workspace from cwd (kept on exit)
  maudebox new feature-x --source trino  # new workspace from another project
  maudebox new feature-x --from main     # start from a specific revision
  maudebox new feature-x --ephemeral     # tear down workspace/overlay on exit
  maudebox new feature-x mvnd verify     # spawn workspace, run a build in it
  maudebox list                          # list registered maudebox instances
  maudebox keep feature-x                # don't tear down on exit
  maudebox rm feature-x                  # full teardown of an instance
  maudebox rm /path/to/myproject         # remove volumes + state for a path
  maudebox --mount ~/.aws:~/.aws:ro      # bind ~/.aws read-only into the container
  maudebox mount add ~/.m2:~/.m2:overlay # persist a mount in the user config
  maudebox alias add cl 'claude --dangerously-skip-permissions --remote-control $MAUDEBOX_INSTANCE'
  maudebox config edit                   # open the user config in $EDITOR
  maudebox config path                   # print the user config path
";

#[derive(Parser)]
#[command(
    name = "maudebox",
    version,
    about = "Launch a maudebox container for a host project directory",
    long_about = LONG_ABOUT,
    after_help = EXAMPLES,
    after_long_help = EXAMPLES,
)]
struct Cli {
    /// Image tag to run.
    #[arg(long, default_value = IMAGE_NAME_DEFAULT)]
    tag: String,

    /// Share Claude's auto-memory dir with another project (host source
    /// keyed to PATH, container target unchanged).
    #[arg(long, value_name = "PATH")]
    memory_from: Option<String>,

    /// Add an extra bind mount (HOST:CONTAINER[:ro|rw|overlay]). Repeatable.
    ///
    /// Leading `~` on the host side expands to $HOME, on the container side
    /// to /root. Mode defaults to rw. Mode `overlay` layers a per-worktree
    /// writable upper (Docker volume) over a read-only host lower — repeat
    /// with different targets to set up multiple overlays. Mounts can also
    /// be declared in $XDG_CONFIG_HOME/maudebox/config.toml.
    #[arg(long, value_name = "SPEC")]
    mount: Vec<String>,

    /// Discriminator appended to $MAUDEBOX_INSTANCE and the container's
    /// `maudebox.instance` label, e.g. `--instance review` on a `trino`
    /// project yields `trino-review`. Use this to run two concurrent
    /// containers on the same project (one for coding, one for review) so
    /// aliases like `claude --remote-control $MAUDEBOX_INSTANCE` don't
    /// collide. Overlay volumes and state dirs are still keyed only on the
    /// project path, so the two sessions share Maven cache state.
    #[arg(long, value_name = "NAME")]
    instance: Option<String>,

    /// Docker network for the container to join at launch (passed to
    /// `docker run --network`). Repeatable to join several networks. Use
    /// this to reach a service running under its own docker-compose project
    /// without publishing any port to the host: attach to the compose-created
    /// network (`<project>_default` by default) and address the service by
    /// its compose service name. Can also be set via the `network` key in
    /// $XDG_CONFIG_HOME/maudebox/config.toml (string or array); any
    /// `--network` flag overrides the config key entirely.
    #[arg(long, value_name = "NAME")]
    network: Vec<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a jj workspace or git worktree and launch maudebox on it.
    New(cmd::new::NewArgs),

    /// List every registered maudebox instance.
    List,

    /// Remove a maudebox-managed instance (workspace, overlay volumes, state dir).
    ///
    /// For projects created by `maudebox new`, this also tears down the
    /// jj workspace / git worktree. For paths handed to `maudebox <path>`
    /// directly, only volumes and state dir are removed — the worktree is
    /// left alone since maudebox didn't create it.
    Rm {
        /// Container ID, instance basename, `new`'s `<name>`, or a project path.
        target: String,
    },

    /// Disarm ephemeral cleanup on a running instance.
    Keep {
        /// Container ID, instance basename, or ephemeral name.
        target: String,
    },

    /// Manage the user config's mounts list.
    Mount(cmd::mount::MountArgs),

    /// Manage the user config's aliases table.
    Alias(cmd::alias::AliasArgs),

    /// Open or locate the user config file.
    Config(cmd::config::ConfigArgs),

    /// (default) Launch a container. First positional is the project dir,
    /// the rest is the container command.
    #[command(external_subcommand)]
    Default(Vec<String>),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            // Print the full chain (e.g. "spawning docker: No such file or
            // directory") so the user sees what actually went wrong rather
            // than just the top-level context label.
            eprintln!("{e:#}");
            ExitCode::from(1)
        }
    }
}

fn dispatch(cli: Cli) -> Result<i32> {
    // Pull out the top-level fields once; each subcommand consumes only what
    // it needs and the default-run case owns the rest.
    let Cli {
        tag,
        memory_from,
        mount,
        instance,
        network,
        command,
    } = cli;
    match command {
        None => default_run(
            tag,
            memory_from,
            mount,
            instance,
            network,
            ".".to_string(),
            Vec::new(),
        ),
        Some(Command::Default(args)) => {
            let mut it = args.into_iter();
            let first = it.next().unwrap_or_else(|| ".".to_string());
            let inner: Vec<String> = it.collect();
            let project_dir = resolve::resolve_project(&first)?;
            default_run(
                tag,
                memory_from,
                mount,
                instance,
                network,
                project_dir,
                inner,
            )
        }
        Some(Command::List) => cmd::list::run(),
        Some(Command::Rm { target }) => cmd::rm::run(&target),
        Some(Command::Keep { target }) => cmd::keep::run(&target),
        Some(Command::Mount(args)) => cmd::mount::run(args.action),
        Some(Command::Alias(args)) => cmd::alias::run(args.action),
        Some(Command::Config(args)) => cmd::config::run(args.action),
        Some(Command::New(args)) => cmd::new::run(args, tag, mount, instance, network),
    }
}

fn default_run(
    image: String,
    memory_from: Option<String>,
    extra_mounts: Vec<String>,
    instance: Option<String>,
    network: Vec<String>,
    project_dir: String,
    command: Vec<String>,
) -> Result<i32> {
    cmd::run::run(cmd::run::RunOptions {
        image,
        memory_from: memory_from.unwrap_or_default(),
        extra_mounts,
        instance: instance.unwrap_or_default(),
        ephemeral_name: String::new(),
        network,
        project_dir,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_command_preserves_hyphenated_inner_arguments() {
        let cli = Cli::try_parse_from(["maudebox", ".", "cargo", "test", "--locked"]).unwrap();
        match cli.command {
            Some(Command::Default(args)) => {
                assert_eq!(args, [".", "cargo", "test", "--locked"])
            }
            _ => panic!("expected default command"),
        }
    }

    #[test]
    fn new_command_parses_ephemeral_and_trailing_command() {
        let cli = Cli::try_parse_from([
            "maudebox",
            "new",
            "feature",
            "--ephemeral",
            "cargo",
            "test",
            "--locked",
        ])
        .unwrap();
        match cli.command {
            Some(Command::New(args)) => {
                assert!(args.ephemeral);
                assert_eq!(args.command, ["cargo", "test", "--locked"]);
            }
            _ => panic!("expected new command"),
        }
    }
}
