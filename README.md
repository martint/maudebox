# maudebox

A Docker image for working on Maven projects with [Claude Code](https://claude.ai/code) in an isolated, reproducible Linux environment. Bundles:

- Eclipse Temurin JDK 25
- [`mvnd`](https://github.com/apache/maven-mvnd) (Maven Daemon) — symlinked as both `mvnd` and `mvn`
- `git` and [`jj`](https://github.com/jj-vcs/jj) (Jujutsu VCS)
- Claude Code CLI

Builds natively on amd64 and arm64.

## What problems does this solve

### Sandboxed Claude Code execution

Running Claude Code directly on your host gives it the same access you have: your entire home directory, your SSH keys, your shell history, every other project on disk, and the ability to run anything `$PATH` exposes. That's a lot of blast radius for an agent that may execute commands you didn't read carefully.

`maudebox` runs Claude inside a container that can only see **the project directory you point it at** (and, for jj workspaces and git worktrees, the base repo it depends on). Everything else — `~/.ssh`, `~/.aws`, sibling projects, your shell config, your browser profile — is simply not mounted and not reachable. The container has no host network privileges beyond what Docker grants, no access to your host's package managers, and no way to install daemons on your machine. Claude can run `mvnd verify`, edit files in the project, run tests, and `jj` / `git` commands against that worktree, but it cannot wander out of it.

Auth material is held back deliberately: `~/.ssh`, `~/.git-credentials`, and `~/.gnupg` are **not** mounted, and `commit.gpgsign` / `tag.gpgSign` are forced off inside the container so a host signing config (e.g. 1Password's macOS-only ssh-sign) doesn't either auto-fail every commit or — worse — get exercised against keys the container shouldn't be able to use.

### No-conflict snapshot publishing across worktrees

Maven's local repository (`~/.m2/repository`) is a shared mutable store. When two builds running concurrently both `mvn install` a snapshot under the same coordinates, the last writer wins and the other build silently picks up the wrong artifact. With multiple Claude sessions iterating on different feature branches in different worktrees of the same project, this is a near-constant footgun: session A installs `1.2.3-SNAPSHOT` from its branch, session B's compile then resolves A's jars, and B's "test failure" has nothing to do with B's code.

`maudebox` gives **each worktree its own writable Maven repository layer** via overlayfs. The host's `~/.m2` is the read-only lower layer, so cached third-party artifacts are shared and warm. The upper layer — where every `install`, every downloaded snapshot, every locally built jar lands — is a Docker named volume keyed to that specific worktree's path. Two concurrent `maudebox` containers on two worktrees of the same repo each see their own `1.2.3-SNAPSHOT`, with zero cross-talk and zero mutation of the host's `~/.m2`.

### Shared Claude login and global config

As a convenience, the host's global Claude config (`CLAUDE.md`, `settings.json`, `agents/`, `commands/`, `plugins/`) is bind-mounted read-only, and the login token is kept in a persistent Docker volume. Log in once inside any container; every future container for any worktree is already logged in.

## Prerequisites

- Docker (tested with OrbStack on macOS; should work with Docker Desktop and native Linux Docker too).
- A host with `~/.m2` (optional — used as a read-only cache layer if present).
- A host with Claude Code installed (optional — its global config under `~/.claude/` is bind-mounted into the container if present).

## Build

```sh
./build.sh
```

Options:

```sh
./build.sh --mvnd-version 1.0.5 --jj-version 0.34.0 --tag maudebox
```

Defaults: `mvnd 1.0.5`, `jj 0.34.0`, image tag `maudebox`.

## Run

The `maudebox` script is the run wrapper. Put it on your `$PATH` (e.g. `ln -s "$PWD/maudebox" ~/.local/bin/maudebox`) or invoke it as `./maudebox` from the repo. The examples below assume it's on `$PATH`.

```sh
maudebox                          # interactive shell, current directory
maudebox /path/to/project         # interactive shell, specific project
maudebox . mvnd verify            # one-shot Maven build
maudebox . claude                 # launch Claude Code
maudebox --clean /path/to/project # delete this worktree's Maven overlay volume
maudebox --tag my-tag . bash      # use a non-default image tag
```

The host project directory is bind-mounted into the container at its **original host path** (e.g. `/Users/martin/projects/trino/workspaces/trino.lateral`). The entrypoint also creates a `/root/<basename>` symlink to that path and starts the shell there, so `$PWD` shows a short, friendly path while the underlying filesystem is still the host-path bind-mount.

If the project is a **jj workspace** or **git worktree**, `maudebox` reads its metadata (`.jj/repo` for jj, `.git` for git), finds the base repo, and bind-mounts it at its host path too — so the absolute paths recorded inside the worktree resolve correctly and `jj` / `git` commands Just Work.

## How it works

### Maven cache

The container mounts an overlayfs at `~/.m2` with three layers:

| Layer    | Source                                      | Mode |
| -------- | ------------------------------------------- | ---- |
| lower    | host's `~/.m2`                              | ro   |
| upper    | per-worktree Docker volume (`m2-upper-…`)   | rw   |
| workdir  | sibling subdir in the same volume           | rw   |

Effect: builds inside the container see all the artifacts already cached on the host, but anything they download or `install` lands in a worktree-scoped volume. Concurrent containers for different worktrees don't collide. The host's `~/.m2` is never mutated.

The per-worktree volume name is derived from the basename of the project directory plus a SHA-256 prefix of its full path:

```
m2-upper-<basename>-<8-char-hash>
```

`maudebox --clean <dir>` removes only that one volume.

### Claude Code config

A shared Docker volume `claude-auth` is mounted at `~/.claude` inside the container. On top of that volume, the following items from the host's `~/.claude/` are bind-mounted read-only (only those that actually exist on the host):

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

This means: **log in to Claude Code once inside any container**, and every future container — for any worktree — will already be logged in.

### Container user

The container runs as **root** (UID 0). This is intentional: OrbStack and similar virtiofs setups root-squash the host bind-mounts, so files in `/m2-host` (and your project source) appear as `uid=0` inside the container. Overlayfs preserves that UID on copy-up, which means a non-root container user can't write to anything pre-existing in the host's `~/.m2`. Running as root sidesteps the whole class of "permission denied on file inherited from the host" failures (mvnd registry, Aether lock files, install-plugin tmp files, etc.).

Everything lives under `/root`: the Maven cache (`/root/.m2`), the Claude config (`/root/.claude`), and a `/root/<basename>` symlink to your worktree's host path. Files written to bind-mounted paths land back on the host owned by your host user, courtesy of virtiofs UID translation.

## Cleanup

```sh
maudebox --clean /path/to/project   # remove that worktree's Maven overlay
docker volume rm claude-auth        # forget the persistent Claude login
docker rmi maudebox                # remove the image
```

## Files

- `Dockerfile` — image definition
- `build.sh` — wrapper around `docker build` with version flags
- `maudebox` — wrapper around `docker run` that wires up all the volume mounts
- `entrypoint.sh` — overlayfs setup + Claude state symlink, then `exec` the user command
- `prompt.sh` — bash prompt with jj/git VCS info, sourced from `/etc/bash.bashrc`
