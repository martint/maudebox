export const USAGE = `Usage: maudebox [--tag TAG] [project-dir] [command...]
       maudebox new <name> [project-dir] [--path PATH] [--from REV] [--ephemeral] [command...]
       maudebox list
       maudebox clean [project-dir]
       maudebox keep <id-or-name>
       maudebox mount {add SPEC | list | rm SPEC}
       maudebox alias {add NAME VALUE | list | rm NAME}

Launch a maudebox container for a host project directory. The project (and
its base repo, if it's a jj workspace or git worktree) is bind-mounted at
its host path inside the container so VCS metadata still resolves. Extra
host paths can be exposed via --mount or the user config; any of them can use
mode \`overlay\` to layer a per-worktree writable upper over a read-only
host lower (typically used for ~/.m2 to isolate Maven snapshots per
worktree without mutating the host cache).

Options:
  --tag TAG            image tag to run (default: maudebox)
  --memory-from PATH   share Claude's auto-memory dir with another project
                       (host source keyed to PATH, container target unchanged)
  --mount SPEC         add an extra bind mount, where SPEC is
                         HOST_PATH:CONTAINER_PATH[:ro|rw|overlay]
                       Leading \`~\` on the host side expands to $HOME, on the
                       container side to /root. Mode defaults to rw. Mode
                       \`overlay\` layers a per-worktree writable upper (Docker
                       volume) over a read-only host lower — repeat with
                       different targets to set up multiple overlays. Mounts
                       can also be declared in
                       $XDG_CONFIG_HOME/maudebox/config.toml (TOML):
                         mounts = [
                             "~/.m2:~/.m2:overlay",
                             "~/.aws:~/.aws:ro",
                         ]
  -h, --help           show this help and exit

Arguments:
  project-dir   host directory to mount (default: current directory)
  command...    command to run inside the container (default: interactive shell)

Subcommands:
  new <name>    create a jj workspace or git worktree from project-dir
                (default: cwd) and launch maudebox on it, sharing the
                parent project's Claude auto-memory dir. The workspace and
                its overlay volume persist after the container exits, just
                like \`maudebox <path>\`. Trailing positional args are passed
                as the container command (default: interactive shell).
                  --path PATH   target path
                                (default: <project>/../<basename>.<name>)
                  --from REV    starting revision
                                (default: jj @-, git HEAD)
                  --ephemeral   tear down the workspace/worktree and its
                                overlay volume when the container exits
                                (use \`maudebox-keep\` / \`maudebox keep\` to
                                disarm mid-session)
  list          list every registered maudebox instance (anything with at
                least one overlay volume), showing its container ID if
                running, its running/stopped status, instance name,
                ephemeral status, overlay count, and project path.
  clean [project-dir]
                remove every overlay volume tied to project-dir
                (default: cwd). Works on a since-deleted worktree too,
                so orphan volumes can still be reclaimed by passing
                the original path.
  keep <id-or-name>
                for a running ephemeral instance, prevent its workspace and
                Maven overlay from being deleted when the container exits.
                The argument matches the container ID (as shown in \`maudebox
                list\`), the instance basename (the path's last component), or
                the ephemeral name originally passed to \`maudebox new\`. From
                inside the container, run \`maudebox-keep\` instead — same
                effect.
  mount add SPEC
                append a mount spec (HOST:CONTAINER[:ro|rw|overlay]) to
                the user config so it applies to every future invocation.
  mount list    print all mount specs currently configured in the user config.
  mount rm SPEC
                remove a previously-added mount spec from the user config.
  alias add NAME VALUE
                define a bash alias inside the container by adding it to
                the [aliases] table in the user config. VALUE can reference
                container env vars like $MAUDEBOX_INSTANCE — they expand
                when the alias is invoked, not when defined. Quote VALUE
                on the shell to keep arguments together.
  alias list    print all aliases currently configured in the user config.
  alias rm NAME
                remove a previously-added alias from the user config.

Examples:
  maudebox                               # interactive shell in current dir
  maudebox /path/to/myproject            # interactive shell, specific project
  maudebox . mvnd verify                 # run a build
  maudebox . claude                      # start Claude Code
  maudebox clean /path/to/myproject      # delete that worktree's overlay volume
  maudebox new feature-x                 # new workspace from cwd (kept on exit)
  maudebox new feature-x /path/to/proj   # new workspace from a specific project
  maudebox new feature-x --from main     # start from a specific revision
  maudebox new feature-x --ephemeral     # tear down workspace/overlay on exit
  maudebox new feature-x mvnd verify     # spawn workspace, run a build in it
  maudebox list                          # list registered maudebox instances
  maudebox keep feature-x                # don't tear down on exit (running instance)
  maudebox --mount ~/.aws:~/.aws:ro      # bind ~/.aws read-only into the container
  maudebox mount add ~/.m2:~/.m2:overlay # persist a mount in the user config
  maudebox mount list                    # show configured mounts
  maudebox mount rm ~/.m2:~/.m2:overlay  # remove a configured mount
  maudebox alias add cl 'claude --dangerously-skip-permissions --remote-control $MAUDEBOX_INSTANCE'
`;
