# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

A Docker-based dev environment that ships JDK 25 (Temurin), `mvnd` (Maven Daemon), `git`, `jj` (jujutsu), and the Claude Code CLI in a single image. The image is meant to be run against an arbitrary host project directory: the host's source tree is bind-mounted at its original host path inside the container (so jj/git worktree metadata resolves), and the host's `~/.m2` is exposed as a read-only lower layer beneath a per-worktree overlayfs upper layer so each worktree gets isolated Maven local-repo writes while still benefiting from the shared host cache.

The repo contains only the container itself: `Dockerfile`, `build.sh`, `maudebox` (the run wrapper), `entrypoint.sh`, `prompt.sh`. There is no application source code here — changes to this repo are changes to the dev-container itself.

## Common commands

Build the image (defaults: `mvnd 1.0.5`, `jj 0.34.0`, tag `maudebox`):

```
./build.sh
./build.sh --mvnd-version 1.0.5 --jj-version 0.34.0 --tag maudebox
```

Run a container against a host project directory:

```
maudebox                          # interactive shell, current dir
maudebox /path/to/proj            # interactive shell, specific project
maudebox . mvnd verify            # one-shot build inside the container
maudebox . claude                 # launch Claude Code inside the container
maudebox --clean /path/to/proj    # delete that worktree's overlay upper volume
```

There are no tests, no linters, and no CI configured in this repo. To validate a change, rebuild the image and exercise the affected path with `maudebox`.

## Architecture

### Three-layer Maven cache via overlayfs

The non-obvious piece is in `entrypoint.sh`. At container start it mounts an `overlay` filesystem at `/root/.m2` with:

- `lowerdir=/m2-host` — the host's `~/.m2`, bind-mounted read-only by `maudebox`.
- `upperdir=/m2-upper/upper` — inside a per-worktree named Docker volume, writable.
- `workdir=/m2-upper/work` — sibling subdir in the same volume.

Both upperdir and workdir live under `/m2-upper` (a Docker named volume backed by the host filesystem) rather than on the container's rootfs. This is mandatory: kernel overlayfs refuses to use another overlay as upperdir/workdir, and Docker's container rootfs is itself overlayfs. Putting them in a named volume sidesteps that.

This means every container sees the host's pre-warmed Maven cache, but writes (downloaded artifacts, locally installed snapshots) go into a volume scoped to that worktree, so concurrent containers for different projects never stomp on each other and the host `~/.m2` is never mutated. Mounting overlayfs from inside a container requires `--cap-add SYS_ADMIN` and `--security-opt apparmor=unconfined`, which `maudebox` supplies. If `/m2-host` is empty or absent, the entrypoint falls back to a plain `~/.m2` rather than failing.

### Container runs as root

The container runs everything as root (UID 0). This is deliberate, not laziness: OrbStack root-squashes virtiofs bind-mounts, so files in `/m2-host` and the host project bind-mount appear as `uid=0` inside the container. When overlayfs copies a file up from lowerdir, it preserves the lowerdir UID — so any path that already existed on the host becomes root-owned in the upperdir. Running the container as a non-root user (we previously tried `ubuntu`/UID 1000 with a `runuser` privilege drop) made `~/.m2` effectively unwritable for anything that already existed in the host's cache: mvnd registry, Aether lock files, install-plugin tmp files, etc. Each manifestation needed its own workaround (env var redirect, tmpfs shadow). Running as root sidesteps the entire class of problems.

Everything lives at `/root`: the Maven cache mount (`/root/.m2`), the Claude config volume (`/root/.claude`), and a `/root/<basename>` symlink to the worktree's host path. No `ENV HOME` override, no passwd massaging — just root's natural home. The `ubuntu` user still exists in the image (from the Noble base) but isn't used.

The entrypoint is correspondingly single-stage: mount overlayfs, set up the Claude state symlink, `exec "$@"`. No `runuser`, no setpriv.

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

### Per-worktree volume naming

`maudebox` derives the upper-layer volume name as `maudebox-overlay-<basename>-<sha256-prefix-of-fullpath>`. The basename keeps it human-readable; the hash prevents collisions when two worktrees share a basename. `--clean` removes only that one volume.

### Multi-arch build

Both `mvnd` and `jj` install steps in the `Dockerfile` branch on `uname -m` to pick `amd64`/`aarch64` artifacts, so the image builds natively on Apple Silicon and x86_64 hosts. `jj` uses the musl static binaries to avoid libc-version coupling to the base image.

### Claude Code install

Claude Code is installed as the native binary via `curl … | bash` and moved into `/usr/local/bin/claude`. The installer drops a launcher symlink into `$HOME/.local/bin/claude` whose target lives in `$HOME/.local/share/claude/versions/<ver>`. The Dockerfile resolves the symlink with `readlink -f` and moves the real binary into `/usr/local/bin`, then removes `$HOME/.local/share/claude` and `$HOME/.claude` so the install dir doesn't bloat the image and `$HOME/.claude` is left empty for the runtime volume mount. After the move, `$HOME/.local/bin/claude` is recreated as a symlink to `/usr/local/bin/claude`: the native binary records its install path as `installMethod=native` (persisted into `~/.claude/state.json` on the named volume) and warns at startup if `~/.local/bin/claude` is missing.

## Conventions for changes in this repo

- Keep the image multi-arch — any new tool install should branch on `uname -m` the same way `mvnd` and `jj` do.
- The container runs as **root** (no `USER` directive), with everything under `/root`. Don't add a `runuser`/setpriv-style privilege drop — that re-introduces the overlay copy-up UID problems described above. The `ubuntu` user from the Noble base is unused at runtime; don't build paths around it.
- Claude Code's installer drops a launcher symlink into `$HOME/.local/bin/claude` whose target lives in `$HOME/.local/share/claude/versions/<ver>`. The Dockerfile resolves the symlink (`readlink -f`) and moves the actual binary into `/usr/local/bin` so it lives outside any volume mount path. Don't replace this with a plain `mv` of the symlink. Keep the `~/.local/bin/claude → /usr/local/bin/claude` symlink — the native binary checks for it at startup and warns if it's missing.
- **Java's `user.home` ≠ `$HOME`.** Java derives `user.home` from `getpwuid()->pw_dir` in `/etc/passwd`, not from the env var. Other tools (Python's `pathlib.Path.home()`, Go's `os.UserHomeDir()`) do the same. With root running and root's pw_dir naturally being `/root`, this works out — but if you ever switch the runtime user or override `HOME`, expect Maven (and friends) to disagree about where `~` lives. Either keep them aligned or pass `-Duser.home=…` explicitly.
- The jj release tarball stores members with a `./` prefix, so `tar -x … ./jj` is required (not `… jj`).
- The overlayfs setup is the load-bearing trick of this image. Don't replace it with a plain bind-mount or a `cp` of `~/.m2` without understanding the isolation guarantees it provides.
- **Overlay copy-up preserves lowerdir UIDs.** When the kernel copies a file from lowerdir to upperdir on first write, it preserves the lowerdir file's ownership. OrbStack root-squashes virtiofs, so everything in lowerdir appears as `uid=0`. Running the container as root makes this a non-issue. `MVND_DAEMON_STORAGE=~/.mvnd` is still set as a small optimization (mvnd state stays out of the overlay's upperdir, avoiding pointless copy-up traffic), but it's no longer load-bearing. If you ever try to drop privileges back to a non-root user, expect failures across the board (mvnd registry, Aether `.locks/`, install-plugin tmp files, etc.) — see the "Container runs as root" section for context.
