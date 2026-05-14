# maudebox

A Docker image for working on Maven projects (and adjacent polyglot bits) with [Claude Code](https://claude.ai/code) in an isolated, reproducible Linux environment. Bundles:

- Eclipse Temurin JDK 25
- [`mvnd`](https://github.com/apache/maven-mvnd) (Maven Daemon) — symlinked as both `mvnd` and `mvn`
- A Rust toolchain (rustup-managed, default 1.85.0) plus `build-essential` for crates with native dependencies
- Python 3 with `pip`, `venv`, and development headers (`python` is symlinked to `python3`)
- [`bun`](https://bun.sh) and [`pnpm`](https://pnpm.io) for JavaScript/TypeScript work (pnpm bundles its own Node.js; bun is its own runtime)
- `git` and [`jj`](https://github.com/jj-vcs/jj) (Jujutsu VCS)
- Claude Code CLI
- GitHub CLI (`gh`)
- `ripgrep`, `vim`, `less`, `sudo`

Builds natively on amd64 and arm64.

## What problems does this solve

### Sandboxed Claude Code execution

Running Claude Code directly on your host gives it the same access you have: your entire home directory, your SSH keys, your shell history, every other project on disk, and the ability to run anything `$PATH` exposes. That's a lot of blast radius for an agent that may execute commands you didn't read carefully.

`maudebox` runs Claude inside a container that can only see **the project directory you point it at** (and, for jj workspaces and git worktrees, the base repo it depends on). Everything else — `~/.ssh`, `~/.aws`, sibling projects, your shell config, your browser profile — is simply not mounted and not reachable. The container has no host network privileges beyond what Docker grants, no access to your host's package managers, and no way to install daemons on your machine. Claude can run `mvnd verify`, edit files in the project, run tests, and `jj` / `git` commands against that worktree, but it cannot wander out of it.

Auth material is held back deliberately: `~/.ssh`, `~/.git-credentials`, and `~/.gnupg` are **not** mounted, and `commit.gpgsign` / `tag.gpgSign` are forced off inside the container so a host signing config (e.g. 1Password's macOS-only ssh-sign) doesn't either auto-fail every commit or — worse — get exercised against keys the container shouldn't be able to use.

### No-conflict snapshot publishing across worktrees

Maven's local repository (`~/.m2/repository`) is a shared mutable store. When two builds running concurrently both `mvn install` a snapshot under the same coordinates, the last writer wins and the other build silently picks up the wrong artifact. With multiple Claude sessions iterating on different feature branches in different worktrees of the same project, this is a near-constant footgun: session A installs `1.2.3-SNAPSHOT` from its branch, session B's compile then resolves A's jars, and B's "test failure" has nothing to do with B's code.

`maudebox` can give **each worktree its own writable Maven repository layer** via overlayfs — opt in by adding an `overlay` mount in your config (see "Extra bind mounts" below; typically `~/.m2:~/.m2:overlay`). The host source becomes the read-only lower layer, so cached third-party artifacts are shared and warm. The upper layer — where every `install`, every downloaded snapshot, every locally built jar lands — is a Docker named volume keyed to that specific worktree's path. Two concurrent `maudebox` containers on two worktrees of the same repo each see their own `1.2.3-SNAPSHOT`, with zero cross-talk and zero mutation of the host's `~/.m2`.

### Shared logins and global config

As a convenience, the host's global Claude config (`CLAUDE.md`, `settings.json`, `agents/`, `commands/`, `plugins/`) is bind-mounted read-only, and login state for Claude and the GitHub CLI is kept in a shared persistent Docker volume. Log in once to each inside any container; every future container for any worktree is already logged in.

## Prerequisites

- Docker (tested with OrbStack on macOS; should work with Docker Desktop and native Linux Docker too).
- A Rust toolchain (stable, 1.75+) to build the wrapper. `cargo` and `rustc` only.
- A host with `~/.m2` (optional — only used if you add an `overlay` mount for it).
- A host with Claude Code installed (optional — its global config under `~/.claude/` is bind-mounted into the container if present).

## Build

The repo is a Cargo workspace. One tool builds both the host-side wrapper and the docker image:

```sh
cargo build --release        # wrapper binary at target/release/maudebox
cargo xtask image            # docker image (defaults: mvnd 1.0.5, jj 0.41.0, tag maudebox)
cargo xtask all              # both, in one go
```

Version pins and tag can be overridden:

```sh
cargo xtask image --mvnd-version 1.0.5 --jj-version 0.41.0 --rust-version 1.85.0 --bun-version 1.3.13 --pnpm-version 11.1.0 --tag maudebox
```

## Run

Install the wrapper somewhere on your `$PATH` — either via `cargo install --path .` (drops it under `~/.cargo/bin/`) or by symlinking the release build (`ln -s "$PWD/target/release/maudebox" ~/.local/bin/maudebox`). The examples below assume it's on `$PATH`.

```sh
maudebox                          # interactive shell, current directory
maudebox /path/to/project         # interactive shell, specific project
maudebox . mvnd verify            # one-shot Maven build
maudebox . claude                 # launch Claude Code
maudebox rm /path/to/project      # tear down an instance (workspace if maudebox created it, plus volumes + state dir)
maudebox --tag my-tag . bash      # use a non-default image tag
maudebox list                     # list registered maudebox instances
maudebox keep feature-x           # don't tear down on exit (running ephemeral)
```

Run `maudebox --help` for the full option list.

The host project directory is bind-mounted into the container at its **original host path** (e.g. `/Users/martin/projects/trino/workspaces/trino.lateral`). The entrypoint also creates a `/root/<basename>` symlink to that path and starts the shell there, so `$PWD` shows a short, friendly path while the underlying filesystem is still the host-path bind-mount.

The same basename is also exposed as `$MAUDEBOX_INSTANCE`, which is convenient as a stable handle for the running container — e.g. `claude --remote-control "$MAUDEBOX_INSTANCE"`.

#### Two concurrent containers on the same repo

If you want a second session on the same project — typical case: one for coding, one for review — pass `--instance NAME` to discriminate it:

```sh
maudebox . claude                         # MAUDEBOX_INSTANCE=trino
maudebox --instance review . claude       # MAUDEBOX_INSTANCE=trino-review
```

The flag is appended to the project basename, not a full override, so the basename stays a stable prefix. The discriminator also threads into the `maudebox.instance` container label, so the two sessions appear as distinct handles to `claude --remote-control` and other tools that key off `$MAUDEBOX_INSTANCE`. Overlay volumes and the per-instance state dir are still keyed only on the project path, so both sessions share Maven cache state — if that's not what you want, use `maudebox new` to spin up a second worktree instead.

If the project is a **jj workspace** or **git worktree**, `maudebox` reads its metadata (`.jj/repo` for jj, `.git` for git), finds the base repo, and bind-mounts it at its host path too — so the absolute paths recorded inside the worktree resolve correctly and `jj` / `git` commands Just Work.

### Extra bind mounts

By default `maudebox` only sets up the project tree and the Claude/gh state volumes. Any other host path you want exposed inside the container — `~/.m2`, `~/.aws`, a notes directory, a shared cache — is declared explicitly.

Two equivalent ways:

- **Command line:** `--mount HOST:CONTAINER[:ro|rw|overlay]`, repeatable.

  ```sh
  maudebox --mount ~/.aws:~/.aws:ro --mount ~/Documents/notes:~/notes
  ```

- **`$XDG_CONFIG_HOME/maudebox/config.toml`** (TOML; defaults to `~/.config/maudebox/config.toml`) — applies to every invocation:

  ```toml
  mounts = [
      "~/.m2:~/.m2:overlay",
      "~/.aws:~/.aws:ro",
      "~/Documents/notes:~/notes",
  ]
  ```

In both cases the spec syntax is `HOST:CONTAINER[:ro|rw|overlay]`. A leading `~` on the host side expands to `$HOME` (your host home); on the container side it expands to `/root` (the container's home, fixed by the entrypoint). Mode defaults to `rw`. CLI mounts are forwarded to the inner instance when used with `maudebox new`.

**Overlay mode** layers a per-worktree writable Docker volume over the read-only host source, giving each worktree isolated writes while sharing the host content as warm starting state — handy for `~/.m2` (Maven), `~/.cargo`, `~/.gradle`, `~/.npm`, etc. Repeat with different targets to set up multiple overlays in one container; each one creates its own per-worktree volume.

#### Managing mounts from the CLI

You can edit `the user config` by hand, but a small subcommand handles the common operations:

```sh
maudebox mount add ~/.m2:~/.m2:overlay     # append to the user config
maudebox mount list                        # print configured specs
maudebox mount rm  ~/.m2:~/.m2:overlay     # remove an exact-match spec
```

`add` is idempotent (a duplicate spec is left as-is). `rm` requires an exact match against what `mount list` shows — copy-paste from `list` rather than retyping. Only the `mounts = [ ... ]` block in `the user config` is rewritten on mutation; comments and any other TOML content elsewhere in the file are preserved verbatim.

### Aliases

Define bash aliases that get installed automatically inside the container at shell startup. Stored in the `[aliases]` table of `the user config`:

```toml
[aliases]
cl = "claude --dangerously-skip-permissions --remote-control $MAUDEBOX_INSTANCE"
build = "mvnd verify"
```

Manage from the CLI:

```sh
maudebox alias add cl 'claude --dangerously-skip-permissions --remote-control $MAUDEBOX_INSTANCE'
maudebox alias list
maudebox alias rm cl
```

Single-quote VALUE on the host so `$MAUDEBOX_INSTANCE` (and any other `$VAR` references) stay literal in the alias definition — they expand each time the alias is invoked, picking up the actual container env. `add` updates the value if the name already exists; `rm` removes by name. Only the `[aliases]` table is rewritten on mutation; the rest of `the user config` is preserved.

Aliases also work when invoked non-interactively via `maudebox <path> <name> <args>`. Bash's built-in alias mechanism only fires for interactive shells, so the wrapper detects an alias name as the first user argument and rewrites the docker run command to `bash -c '<value> "$@"' <name> <args>` — same expansion semantics as the interactive case, with trailing args forwarded through `$@`.

### Reaching host services

The container can reach services running on the host (e.g. an HTTP MCP server, a local dev API) at `host.docker.internal`. `maudebox` passes `--add-host=host.docker.internal:host-gateway` to `docker run` so this resolves on Linux as well as macOS/OrbStack. From inside the container, point at `http://host.docker.internal:<port>`; `localhost` still means the container itself.

For an HTTP MCP server in particular, either drop a `.mcp.json` at the project root with the URL (it's bind-mounted in and Claude picks it up automatically), or run `claude mcp add --transport http <name> http://host.docker.internal:<port>` once — that persists in the shared state volume and is available in every future container.

### Managed MCP servers

If you want one or more MCP servers available in **every** maudebox container with no per-container `claude mcp add`, declare them in the user config as `[mcp.NAME]` tables:

```toml
[mcp.my_http]
type = "http"
url = "http://host.docker.internal:8080/mcp"

[mcp.my_stdio]
type = "stdio"
command = "python"
args = ["-m", "my_server"]

[mcp.my_stdio.env]
DEBUG = "1"
```

`maudebox` serializes the table to `$XDG_STATE_HOME/maudebox/managed-mcp.json` on each launch and bind-mounts it read-only at `/etc/claude-code/managed-mcp.json` inside the container. Claude Code reads that path as **managed scope**, so every session loads these servers automatically — they show up in `claude mcp list` and don't need `claude mcp add`. The field set inside each `[mcp.NAME]` table is passed through verbatim (validation is Claude's job); see Claude Code's MCP docs for the available fields (`type`, `url`, `command`, `args`, `env`, `headers`, …).

To reach a server running on the host, use `host.docker.internal` as shown above (see *Reaching host services*). Remove or comment out the table to drop the server — the file isn't written when no `[mcp.*]` tables are present, and managed scope is read-only inside the container (users can't disable an entry via `claude mcp remove`).

Verify inside the container with `claude mcp list` — managed entries are loaded automatically:

```
$ claude mcp list
my_http: http://host.docker.internal:8080/mcp (HTTP) - ✓ Connected
my_stdio: python -m my_server (stdio) - ✓ Connected
```

Edits to the user config only take effect for **new** containers; an already-running container keeps using the snapshot of the file it was launched with. Exit and relaunch `maudebox` to pick up changes.

### Spawning a new workspace or worktree

`maudebox new <name>` creates a fresh jj workspace or git worktree off an existing project and launches `maudebox` on it. By default the workspace persists after the container exits, just like running `maudebox <path>` would — `new` is essentially `mkdir-and-enter`. Pass `--ephemeral` for short-lived scratch workspaces that should be torn down on exit.

```sh
maudebox new feature-x                       # new workspace from cwd
maudebox new feature-x /path/to/project      # new workspace off a specific project
maudebox new feature-x --from main           # start from a specific revision
maudebox new feature-x --path /tmp/scratch   # custom target path
maudebox new feature-x --ephemeral           # tear down workspace + overlay on exit
```

Defaults:

- **Target path:** `<project>/../<basename>.<name>` (sibling of the source project). Override with `--path PATH`.
- **Starting revision:** jj's `@-` for jj, `HEAD` for git. Override with `--from REV`. For git, a new branch named `<name>` is created at that revision.
- **VCS choice:** jj is preferred when both `.jj/` and `.git` are present (colocated repos).

#### Reattaching to a non-ephemeral workspace

After `maudebox new <name>` exits (without `--ephemeral`), the workspace persists at its sibling path. You don't need to remember the path to re-launch it — `maudebox <name>` looks the name up in the manifest store and reattaches:

```sh
maudebox new feature-x         # creates worktree at <project>/../<basename>.feature-x
exit
maudebox feature-x             # re-enters the same workspace
maudebox feature-x mvnd verify # …or run a one-shot command in it
```

Name lookup is only attempted when the first positional doesn't look like a path (no leading `/`, `.`, `~` and no embedded `/`). A bare name has to match exactly one workspace whose target directory still exists on disk; ambiguous matches across projects bail out asking for the full path, and a miss falls through to the regular `<path>` handling. Workspaces created by older maudebox versions (before the worktree path was recorded in the manifest) aren't reattachable by name — keep launching them via `maudebox <path>` until they're torn down.

#### Ephemeral mode

With `--ephemeral`, the wrapper tears the workspace down once the container exits — on a clean exit, an error, or Ctrl-C alike. The teardown delegates to `maudebox rm`, which reads the manifest `new` wrote into the per-instance state dir and:

- **jj:** runs `jj workspace forget <name>` and `rm -rf <target>`. The change at `@` in that workspace becomes a regular commit in jj's op log, so committed work is recoverable via `jj op log` / `jj op restore`.
- **git:** runs `git worktree remove --force <target>` and `git branch -D <name>`. Branch tip commits remain reachable via the reflog (default 90 days).
- **Overlay volume:** the per-worktree `maudebox-overlay-…` volume is removed.
- **State dir:** the per-instance state dir under `$XDG_STATE_HOME/maudebox/instances/…` is removed last.

Uncommitted working-copy changes are **not** preserved. Commit before exiting the container if you might want them later.

##### Changing your mind mid-session

If you've launched an ephemeral instance and later decide you'd rather hold onto it, you can disarm the cleanup without leaving the running container:

- **From inside the container:** run `maudebox-keep`.
- **From the host:** run `maudebox keep <id-or-name>`, where the argument is the container ID (as shown by `maudebox list`), the instance basename, or the original name passed to `maudebox new`.

Either form drops a `keep` flag in the state dir. On exit, the wrapper sees the flag, removes only the flag itself (the manifest stays in place), and leaves the workspace, overlay volume, and state dir intact — so a later `maudebox rm <name>` still recognizes the workspace as maudebox-managed and can do the full teardown when you really are done with it.

Either way, when the container exits its workspace and Maven overlay are left in place instead of being deleted. Both are no-ops for non-ephemeral instances. After the container exits, a kept workspace is just a regular jj workspace / git worktree.

## How it works

### Overlay mounts (typically the Maven cache, optionally more)

For each mount spec that uses mode `overlay` (e.g. `~/.m2:~/.m2:overlay`), the container gets an overlayfs at the target path with three layers:

| Layer    | Source                                                  | Mode |
| -------- | ------------------------------------------------------- | ---- |
| lower    | the host source path                                    | ro   |
| upper    | per-worktree+per-target Docker volume (`maudebox-overlay-…-<target-hash>`) | rw   |
| workdir  | sibling subdir in the same volume                       | rw   |

Effect: builds inside the container see all the artifacts already cached on the host, but anything they download or `install` lands in a worktree-scoped volume. Concurrent containers for different worktrees don't collide. The host source is never mutated. You can declare more than one — e.g. one for Maven, one for Cargo — and each gets its own per-worktree volume. Without any `overlay` mount declared, no overlay is set up and no volume is created — `maudebox` is just a project bind-mount + state volumes.

`maudebox list` aggregates by project and shows an `OVERLAYS` count column. `maudebox rm <id-or-name-or-path>` removes every overlay volume tied to that project, drops the per-instance state dir, and — if the state dir holds a manifest left by `maudebox new` — also tears down the jj workspace / git worktree. A path handed to `maudebox <path>` directly has no manifest, so `rm` leaves the worktree alone and only cleans up what maudebox itself created.

The per-worktree volume name is derived from the basename of the project directory plus a SHA-256 prefix of its full path:

```
maudebox-overlay-<basename>-<8-char-hash>
```

### Login state and global config

A shared Docker volume `maudebox-state` holds writable state across containers, with two isolated subtrees mounted at the canonical paths each tool expects:

- `claude/` → `/root/.claude` (Claude login, plugin caches)
- `gh/` → `/root/.config/gh` (`gh auth` token, config)

On top of the `claude/` subtree, the following items from the host's `~/.claude/` are bind-mounted read-only (only those that actually exist on the host):

- `CLAUDE.md` — your global instructions
- `settings.json`
- `agents/`
- `commands/`
- `plugins/`

Items that are keyed to host paths or are session-only state (`projects/`, `todos/`, `statsig/`, `shell-snapshots/`) are intentionally **not** mounted.

`~/.claude.json` (Claude's login token and project state) lives outside `~/.claude/`, so the entrypoint symlinks it into the persistent volume:

```
~/.claude.json -> ~/.claude/state.json
```

This means: **log in to Claude Code (and `gh`) once inside any container**, and every future container — for any worktree — will already be logged in.

### Container user

The container runs as **root** (UID 0) on macOS. This is intentional: OrbStack and similar virtiofs setups root-squash the host bind-mounts, so files in the lower layer of an overlay mount appear as `uid=0` inside the container. Overlayfs preserves that UID on copy-up, which means a non-root container user couldn't write to anything pre-existing in the host source. Running as root sidesteps the whole class of "permission denied on file inherited from the host" failures (mvnd registry, Aether lock files, install-plugin tmp files, etc.). On Linux the entrypoint drops privileges to the host UID after the privileged setup steps are done.

Everything lives under `/root`: the Claude config (`/root/.claude`), an opt-in overlay target (typically `/root/.m2`), and a `/root/<basename>` symlink to your worktree's host path. Files written to bind-mounted paths land back on the host owned by your host user, courtesy of virtiofs UID translation (macOS) or the privilege drop (Linux).

Because of the Linux privilege drop, in-container privileged operations (`apt-get install`, `mount`, etc.) fail with `Permission denied`. The image installs `sudo` and grants the runtime user passwordless sudo, so the escape hatch is `sudo apt-get install …` (or `sudo -i` for a root shell). The rule is inert on macOS/OrbStack where the session stays root anyway.

## Cleanup

```sh
maudebox rm /path/to/project        # tear down an instance (workspace if maudebox created it, plus volumes + state dir)
docker volume rm maudebox-state     # forget persistent Claude + gh logins
docker rmi maudebox                 # remove the image
```

## Layout

- `Cargo.toml`, `src/` — the host-side `maudebox` wrapper (single binary, no runtime deps).
- `xtask/` — small Rust helper crate wired in as `cargo xtask`. Drives `docker build` (and, for `all`, also `cargo build --release`).
- `.cargo/config.toml` — defines the `xtask` alias so `cargo xtask <subcommand>` works.
- `docker/` — everything the image is built from:
    - `Dockerfile` — image definition.
    - `entrypoint.sh` — overlayfs setup, UID drop on Linux, Claude state symlink, then `exec` the user command.
    - `prompt.sh` — bash prompt with jj/git VCS info, sourced from `/etc/bash.bashrc`.
    - `aliases.sh` — installs entries from `MAUDEBOX_ALIASES` as interactive shell aliases.
    - `maudebox-keep` — in-container script for disarming ephemeral cleanup.
