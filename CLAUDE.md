# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

A Docker-based dev environment that ships JDK 25 (Temurin), `mvnd` (Maven Daemon), `git`, `jj` (jujutsu), and the Claude Code CLI in a single image. The image is meant to be run against an arbitrary host project directory: the host's source tree is bind-mounted at its original host path inside the container (so jj/git worktree metadata resolves). Extra host paths can be exposed via `--mount` flags or a `$XDG_CONFIG_HOME/maudebox/config.toml` config; one of them can opt into mode `overlay`, which layers a per-worktree writable upper over a read-only host lower (the typical use case is `~/.m2`, giving each worktree isolated Maven snapshot writes while sharing the host cache as warm starting state).

The repo is a Cargo workspace that builds two things from one tool:

- The host-side `maudebox` wrapper — main package at the repo root (`Cargo.toml`, `src/`).
- The docker image — recipe + COPYed-in container scripts under `docker/` (`Dockerfile`, `entrypoint.sh`, `prompt.sh`, `aliases.sh`, `maudebox-keep`). The image is driven by the `xtask` helper crate (`xtask/`), wired as a cargo subcommand via `.cargo/config.toml`.

There is no application source code here — changes to this repo are changes to the dev-container itself.

## Common commands

Build the wrapper binary (drops it at `target/release/maudebox`):

```
cargo build --release
```

Build the image (defaults: `mvnd 1.0.5`, `jj 0.41.0`, tag `maudebox`):

```
cargo xtask image
cargo xtask image --mvnd-version 1.0.5 --jj-version 0.41.0 --tag maudebox
```

Build everything (wrapper + image) in one go:

```
cargo xtask all
```

Run a container against a host project directory:

```
maudebox                          # interactive shell, current dir
maudebox /path/to/proj            # interactive shell, specific project
maudebox . mvnd verify            # one-shot build inside the container
maudebox . claude                 # launch Claude Code inside the container
maudebox rm /path/to/proj         # tear down an instance (workspace if maudebox created it, plus overlay volumes + state dir)
```

`cargo test` runs the wrapper's unit tests (currently a small parity-with-bash check on the volume-name hashing). To validate a change end-to-end, rebuild the image (`cargo xtask image`) and exercise the affected path with `maudebox`.

## Architecture

### Opt-in three-layer cache via overlayfs (one or more)

The non-obvious piece is in `entrypoint.sh`. For each mount spec that uses mode `overlay` (e.g. `~/.m2:~/.m2:overlay` in `$XDG_CONFIG_HOME/maudebox/config.toml` or via `--mount`), the wrapper:

- bind-mounts the host source at `/maudebox/overlay-N/lower` read-only,
- creates a per-worktree+per-target Docker volume (`maudebox-overlay-<basename>-<projhash>-<targethash>`) mounted at `/maudebox/overlay-N/upper`,
- appends a `lower:upper:target` triple to the `MAUDEBOX_OVERLAYS` env var.

The entrypoint loops over `MAUDEBOX_OVERLAYS` and mounts an `overlay` filesystem at each container target with:

- `lowerdir=<lower>` — the host source, read-only.
- `upperdir=<upper>/upper` — inside the per-overlay volume, writable.
- `workdir=<upper>/work` — sibling subdir in the same volume.

Both upperdir and workdir live inside the per-overlay Docker volume (host-fs backed) rather than on the container's rootfs. This is mandatory: kernel overlayfs refuses to use another overlay as upperdir/workdir, and Docker's container rootfs is itself overlayfs. Putting them in a named volume sidesteps that.

The typical use case is the host's `~/.m2`: every container sees the pre-warmed Maven cache, but writes (downloaded artifacts, locally installed snapshots) go into a volume scoped to that worktree, so concurrent containers for different projects never stomp on each other and the host source is never mutated. Multi-overlay lets you do the same for `~/.cargo`, `~/.gradle`, `~/.npm`, etc. — one overlay per polyglot tool. Mounting overlayfs from inside a container requires `--cap-add SYS_ADMIN` and `--security-opt apparmor=unconfined`, which `maudebox` supplies. If a lowerdir is empty or absent, the entrypoint logs a notice and skips that overlay rather than failing. Without any overlay specs, `MAUDEBOX_OVERLAYS` is unset and the entrypoint's overlay loop is a no-op.

`maudebox list` aggregates by project (one row per `maudebox.project` label across all that project's overlay volumes, with an `OVERLAYS` count column). `maudebox rm <id-or-name-or-path>` removes every volume labelled with that project plus the per-instance state dir, and — if the state dir holds a manifest left by `maudebox new` — also tears down the jj workspace / git worktree. For a path handed to `maudebox <path>` directly, the worktree is left alone since maudebox didn't create it; only the volumes and state dir come off.

### Container runs as root, then drops privileges on Linux

The container starts as root (UID 0) and stays that way for the privileged setup steps — overlayfs mount needs `CAP_SYS_ADMIN`, and the volume mounts are root-owned at first creation. After that, the entrypoint's behavior diverges by host platform:

- **macOS / OrbStack:** stays root for the entire session. OrbStack root-squashes virtiofs bind-mounts, so files in any overlay's lowerdir and the host project bind-mount appear as `uid=0` inside the container, and writes from the container's root user translate back to the host user transparently. Running as anything other than root would break overlayfs copy-up: lowerdir files appear as `uid=0`, copy-up preserves lowerdir UID, so the upperdir becomes root-owned and a dropped-privilege user can't write to it. We tried this once with `ubuntu`/UID 1000 + `runuser` — mvnd registry, Aether lock files, install-plugin tmp files all broke and each needed its own workaround.

- **Linux Docker:** entrypoint drops privileges to the host UID/GID before `exec`'ing the user command. Linux bind mounts pass UIDs through literally — there is no root-squash — so a root-in-container write into the project tree lands as a root-owned file on the host. The wrapper passes `HOST_UID`/`HOST_GID`; the entrypoint rewrites `/etc/passwd` so that UID's home is `/root` (Java's `user.home` reads pw_dir, not `$HOME`), `chown`s the writable trees (every `/maudebox/overlay-*/upper`, `/root/.claude`, `/root/.config/gh`, `/root` itself) using `find -xdev ! -uid $HOST_UID` so it's idempotent and doesn't try to touch read-only bind mounts on a different fs, then `exec setpriv --reuid=… --regid=… --clear-groups -- "$@"`. The OrbStack-specific copy-up problem doesn't occur here because the host source is already host-uid-owned, so copy-up produces host-uid-owned upperdir files that the dropped-privilege process can write.

Everything still lives at `/root` regardless of platform: the Claude config volume (`/root/.claude`), an opt-in overlay target (typically `/root/.m2`), and a `/root/<basename>` symlink to the worktree's host path. The `ubuntu` user from the noble base may collide with `HOST_UID=1000` on Linux, which is why the entrypoint *replaces* any existing passwd entry for `HOST_UID` rather than appending — otherwise `/home/ubuntu` would win as pw_dir.

### jj workspaces / git worktrees

Both jj workspaces and git worktrees store an absolute (or relative-to-cwd) path to the *base* repo inside the worktree's metadata: `<workspace>/.jj/repo` is a file containing the path to the main repo's `.jj/repo` directory; `<worktree>/.git` is a file containing `gitdir: <abs path>` to the main's `.git/worktrees/<name>`. If we just mount the worktree at some unrelated container path, those references point into nothing and `jj` / `git` fail with confusing errors.

`maudebox` handles this:

1. `detect_vcs_base()` reads `.jj/repo` or `.git`, resolves any relative component, and prints the base repo's working-tree path.
2. The project (and base repo, if different) are bind-mounted into the container at their **host paths**, so `/Users/martin/projects/trino/workspaces/trino.lateral` and `/Users/martin/projects/trino/trino` both exist inside.
3. `maudebox` passes `HOST_PROJECT_DIR=<host path>` as an env var; the entrypoint creates a `/root/<basename of worktree> → <host worktree path>` symlink and `cd`s into it before `exec`'ing the user command. The shell ends up in `/root/<basename>` (short, friendly path) while the underlying filesystem is the host-path bind-mount, so jj/git metadata still resolves.

Edge cases: a regular `git` checkout (`.git` is a directory) or a non-VCS dir is handled by the detection returning empty, in which case only the project itself is bind-mounted.

### Shell prompt (jj/git aware)

`prompt.sh` is `COPY`'d to `/etc/profile.d/dev-prompt.sh` and sourced from `/etc/bash.bashrc`. It defines a `__dev_prompt_vcs` function used as `PROMPT_COMMAND` and a two-line `PS1` of the form `cyan/path  yellow/(vcs)\n green/#`.

For VCS info: it tries jj first (`jj workspace root` → if inside, look up change ID and closest bookmark), and falls back to git's symbolic-ref/short-hash. The `closest_bookmark(to) = heads(::to & bookmarks())` revset alias is passed inline via `jj log --config=...` so we don't need to mount the user's jj config. The whole thing is a stripped-down bash port of the host's powerlevel10k `prompt_my_jj` segment — same data, no zsh/p10k/Nerd Font dependency.

### Host git/jj config sharing

`maudebox` bind-mounts the host's git and jj config files read-only so the user's identity, aliases, ignore rules, and revset aliases (e.g. `closest_bookmark`) are available inside the container:

- `~/.gitconfig`
- `~/.config/git/` (the whole dir, e.g. `ignore`, `attributes`)
- `~/.config/jj/config.toml` only — **not** the whole `~/.config/jj/` dir, because `repos/` underneath is jj's per-repo state cache keyed by host paths.

Auth and signing material (`~/.ssh/`, `~/.git-credentials`, `~/.gnupg/`) is intentionally not mounted.

The host's `commit.gpgsign = true` paired with 1Password's macOS-only ssh-sign program would auto-fail every container commit. The Dockerfile sets `GIT_CONFIG_COUNT/KEY/VALUE` env vars to force `commit.gpgsign=false` and `tag.gpgSign=false` at runtime; these override mounted user config (same precedence as `git -c`).

### Persistent state (Claude + gh)

`maudebox` shares a single named volume `maudebox-state` across all containers/worktrees. The volume hosts two isolated subtrees, each mounted at the canonical path the tool expects via Docker's `volume-subpath`:

- `claude/` → `/root/.claude` (Claude login, plugin caches)
- `gh/`     → `/root/.config/gh` (gh auth, config)

The two trees never mix on disk despite living in the same volume. `volume-subpath` mounts fail if the subdir doesn't exist, so the wrapper runs a throwaway `mkdir -p /v/claude /v/gh` container before each launch (idempotent, ~50ms). Requires Docker 25+ (subpath mounts).

On top of the `claude/` subtree, specific files from the host's `~/.claude/` (`CLAUDE.md`, `settings.json`, `agents/`, `commands/`, `plugins/`) are bind-mounted read-only — picking up the user's global Claude config without dragging in host-path-keyed state (`projects/`, `todos/`, `statsig/`, `shell-snapshots/`).

One narrow carve-out under `projects/`: Claude Code's auto-memory directory `~/.claude/projects/<encoded-cwd>/memory/` is bind-mounted read-write so memories written inside the container reach the host (and vice versa). The encoding maps `/` and `.` in the canonical cwd to `-`, and because the project is bind-mounted at its host path inside the container, host and container normally agree on the key. The rest of `projects/<key>/` (session logs, etc.) is deliberately left in the named volume.

`--memory-from PATH` overrides the *host* side of that bind-mount — the container target stays keyed to the project's cwd (which is what Claude Code looks up inside the container), but the host source is keyed to `PATH` instead. `maudebox new` uses this to point an ephemeral workspace at its parent project's memory, so memories from short-lived workspaces accrue against the long-lived project rather than scattering into per-workspace dirs.

`~/.claude.json` (login token + project list) lives outside `~/.claude/` on the host, so it can't be picked up by the volume mount. The entrypoint instead symlinks `~/.claude.json → ~/.claude/state.json` so writes follow into the persistent volume. The user must log into Claude Code once inside any container; subsequent containers share that login. Same applies to `gh auth login`.

### Container labels and the `keep` flag for ephemeral instances

Every container is stamped with `maudebox.*` labels at run time so the wrapper can find its own containers without parsing image names: `maudebox.instance` (project basename), `maudebox.project` (host path), `maudebox.ephemeral=true|false`. For ephemeral runs (`maudebox new …` without `--keep`), two more labels are set: `maudebox.ephemeral-name` (the user-supplied name) and `maudebox.state-dir` (host path of the per-instance state dir).

The state dir lives at `${XDG_STATE_HOME:-~/.local/state}/maudebox/instances/<volume-name>/` (keyed off the same volume name as the overlay). `maudebox new` always creates it and writes a `manifest.toml` recording the workspace kind, source repo, and `<name>` — its presence is what tells `maudebox rm` that maudebox itself created the worktree (vs. a path the user handed to `maudebox <path>` directly). For ephemeral runs the dir is also bind-mounted at `/run/maudebox/` inside the container so the in-container `maudebox-keep` script (and the host-side `maudebox keep <name>`) can drop a `keep` file. On exit the ephemeral cleanup reads `<state-dir>/keep`: if present, only the keep flag is removed (the manifest stays, so a future `rm` still recognizes the workspace as maudebox-owned); otherwise `rm` is invoked to do the full teardown.

`cmd::new` calls `cmd::run::run` in-process when launching the inner container, so the ephemeral state (name, source, kind) doesn't need to travel through CLI args — it's held in the call stack and persisted via the manifest. There is no public flag for "launch this inner container as ephemeral"; the right way to spawn ephemerals is `maudebox new --ephemeral`.

### Per-worktree volume naming

`maudebox` derives the upper-layer volume name as `maudebox-overlay-<basename>-<sha256-prefix-of-fullpath>`. The basename keeps it human-readable; the hash prevents collisions when two worktrees share a basename. `maudebox rm` removes only that one project's volumes.

### Multi-arch build

Both `mvnd` and `jj` install steps in the `Dockerfile` branch on `uname -m` to pick `amd64`/`aarch64` artifacts, so the image builds natively on Apple Silicon and x86_64 hosts. `jj` uses the musl static binaries to avoid libc-version coupling to the base image.

### Claude Code install

Claude Code is installed as the native binary via `curl … | bash` and moved into `/usr/local/bin/claude`. The installer drops a launcher symlink into `$HOME/.local/bin/claude` whose target lives in `$HOME/.local/share/claude/versions/<ver>`. The Dockerfile resolves the symlink with `readlink -f` and moves the real binary into `/usr/local/bin`, then removes `$HOME/.local/share/claude` and `$HOME/.claude` so the install dir doesn't bloat the image and `$HOME/.claude` is left empty for the runtime volume mount. After the move, `$HOME/.local/bin/claude` is recreated as a symlink to `/usr/local/bin/claude`: the native binary records its install path as `installMethod=native` (persisted into `~/.claude/state.json` on the named volume) and warns at startup if `~/.local/bin/claude` is missing.

## Conventions for changes in this repo

- Keep the image multi-arch — any new tool install should branch on `uname -m` the same way `mvnd` and `jj` do.
- The container runs as **root** (no `USER` directive), with everything under `/root`. The entrypoint may drop privileges to the host UID at the end on Linux, but the privileged setup (overlay mount, `/etc/passwd` rewrite, chowns) still runs as root. Don't add a `USER` directive or replace the entrypoint's `setpriv` step with a docker-CLI `--user` flag — that would break the overlay mount, which needs real-root `CAP_SYS_ADMIN` in the user namespace.
- Claude Code's installer drops a launcher symlink into `$HOME/.local/bin/claude` whose target lives in `$HOME/.local/share/claude/versions/<ver>`. The Dockerfile resolves the symlink (`readlink -f`) and moves the actual binary into `/usr/local/bin` so it lives outside any volume mount path. Don't replace this with a plain `mv` of the symlink. Keep the `~/.local/bin/claude → /usr/local/bin/claude` symlink — the native binary checks for it at startup and warns if it's missing.
- **Java's `user.home` ≠ `$HOME`.** Java derives `user.home` from `getpwuid()->pw_dir` in `/etc/passwd`, not from the env var. Other tools (Python's `pathlib.Path.home()`, Go's `os.UserHomeDir()`) do the same. With root running and root's pw_dir naturally being `/root`, this works out — but if you ever switch the runtime user or override `HOME`, expect Maven (and friends) to disagree about where `~` lives. Either keep them aligned or pass `-Duser.home=…` explicitly.
- The jj release tarball stores members with a `./` prefix, so `tar -x … ./jj` is required (not `… jj`).
- The overlayfs setup is the load-bearing trick when an `overlay` mount is requested. Don't replace it with a plain bind-mount or a `cp` of the source without understanding the isolation guarantees it provides.
- **Overlay copy-up preserves lowerdir UIDs.** When the kernel copies a file from lowerdir to upperdir on first write, it preserves the lowerdir file's ownership. On macOS/OrbStack, virtiofs is root-squashed so everything in lowerdir appears as `uid=0` — which is why the container has to stay root there; dropping privileges would leave the upperdir unwritable for the very files mvnd/Aether/Maven plugins need to update. On Linux, lowerdir files are owned by the host user (no squash), so copy-up produces host-uid-owned upperdir files and the dropped-privilege process can write them — that's why the Linux drop-privileges path doesn't trip the same failures. `MVND_DAEMON_STORAGE=~/.mvnd` keeps mvnd state out of the overlay's upperdir to avoid pointless copy-up traffic — small optimization, not load-bearing.
