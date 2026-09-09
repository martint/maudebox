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
mod state;
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
host lower. Maven cache sharing and local-install isolation are automatic.";

const EXAMPLES: &str = "Examples:
  maudebox                               # interactive shell in current dir
  maudebox /path/to/myproject            # interactive shell, specific project
  maudebox . mvnd verify                 # run a build
  maudebox . claude                      # start Claude Code
  maudebox --instance review . claude    # second session on same repo (distinct $MAUDEBOX_INSTANCE)
  maudebox --state work . claude          # use an independent login/session namespace
  maudebox new feature-x                 # new workspace from cwd (kept on exit)
  maudebox new feature-x --source trino  # new workspace from another project
  maudebox new feature-x --from main     # start from a specific revision
  maudebox new feature-x --ephemeral     # tear down workspace/volumes on exit
  maudebox new feature-x mvnd verify     # spawn workspace, run a build in it
  maudebox list                          # list maudebox projects
  maudebox state list                    # list persistent state namespaces
  maudebox state rm work                 # forget a namespace's credentials and sessions
  maudebox keep feature-x                # don't tear down on exit
  maudebox rm feature-x                  # full teardown of a project
  maudebox rm /path/to/myproject         # remove volumes + state for a path
  maudebox --mount ~/.aws:~/.aws:ro      # bind ~/.aws read-only into the container
  maudebox --fuse . bash                 # allow FUSE mounts (sshfs, rclone, …) inside
  maudebox mount add ~/.gradle:~/.gradle:overlay # persist an overlay mount
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

    /// Persistent state namespace for agent logins, sessions, gh, SSH, and
    /// opencode. CLI selection overrides `state` in the user config; the
    /// default namespace preserves the existing `maudebox-state` volume.
    #[arg(long, value_name = "NAME")]
    state: Option<String>,

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

    /// Allow FUSE filesystems (sshfs, rclone, gocryptfs, …) to be mounted
    /// from inside the container. Exposes `/dev/fuse` and adds the same
    /// `SYS_ADMIN` capability and AppArmor exemption an `overlay` mount
    /// needs, so it is off by default. The image ships `fusermount3`; the
    /// client itself is an in-container `sudo apt-get install …` away. Can
    /// also be set for every launch with `fuse = true` in
    /// $XDG_CONFIG_HOME/maudebox/config.toml.
    #[arg(long)]
    fuse: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a jj workspace or git worktree and launch maudebox on it.
    New(cmd::new::NewArgs),

    /// List every maudebox project, and the instances running against each.
    List,

    /// Remove a maudebox project (its volumes, state dir, and workspace).
    ///
    /// Everything removed is project-scoped, so this affects every instance
    /// of that project; it is refused while any of them is running. For
    /// projects created by `maudebox new`, this also tears down the jj
    /// workspace / git worktree. For paths handed to `maudebox <path>`
    /// directly, only volumes and state dir are removed — the worktree is
    /// left alone since maudebox didn't create it.
    Rm {
        /// Project path or basename, `new`'s `<name>`, or a running
        /// container's ID or instance handle.
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

    /// List or remove persistent state namespaces.
    State(cmd::state::StateArgs),

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
        state,
        network,
        fuse,
        command,
    } = cli;
    let launch = cmd::run::LaunchOptions {
        image: tag,
        memory_from,
        extra_mounts: mount,
        instance,
        state,
        network,
        fuse,
    };
    match command {
        None => default_run(launch, ".".to_string(), Vec::new()),
        Some(Command::Default(args)) => {
            let mut it = args.into_iter();
            let first = it.next().unwrap_or_else(|| ".".to_string());
            let inner: Vec<String> = it.collect();
            let project_dir = resolve::resolve_project(&first)?;
            default_run(launch, project_dir, inner)
        }
        Some(Command::List) => cmd::list::run(),
        Some(Command::Rm { target }) => cmd::rm::run(&target),
        Some(Command::Keep { target }) => cmd::keep::run(&target),
        Some(Command::Mount(args)) => cmd::mount::run(args.action),
        Some(Command::Alias(args)) => cmd::alias::run(args.action),
        Some(Command::Config(args)) => cmd::config::run(args.action),
        Some(Command::State(args)) => cmd::state::run(args.action),
        Some(Command::New(args)) => cmd::new::run(args, launch),
    }
}

fn default_run(
    launch: cmd::run::LaunchOptions,
    project_dir: String,
    command: Vec<String>,
) -> Result<i32> {
    cmd::run::run(cmd::run::RunOptions {
        image: launch.image,
        memory_from: launch.memory_from.unwrap_or_default(),
        extra_mounts: launch.extra_mounts,
        instance: launch.instance.unwrap_or_default(),
        state: launch.state.unwrap_or_default(),
        ephemeral_name: String::new(),
        network: launch.network,
        fuse: launch.fuse,
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

    #[test]
    fn state_selector_applies_to_direct_and_new_launches() {
        let direct = Cli::try_parse_from(["maudebox", "--state", "work", "."]).unwrap();
        assert_eq!(direct.state.as_deref(), Some("work"));

        let new =
            Cli::try_parse_from(["maudebox", "--state", "personal", "new", "feature"]).unwrap();
        assert_eq!(new.state.as_deref(), Some("personal"));
        assert!(matches!(new.command, Some(Command::New(_))));

        let management = Cli::try_parse_from(["maudebox", "state", "rm", "work"]).unwrap();
        assert!(matches!(
            management.command,
            Some(Command::State(cmd::state::StateArgs {
                action: cmd::state::StateAction::Rm { name }
            })) if name == "work"
        ));
    }
}
