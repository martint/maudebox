# Maudebox — Product Specification

This document describes how Maudebox behaves as a product: the problem it
solves, the concepts it asks users to learn, the workflows it supports, and
the boundaries it deliberately keeps. It describes user-visible behavior
rather than implementation detail. For implementation constraints and the
reasons behind them, see `CLAUDE.md` and `docs/decisions/`.

Where this document and the implementation disagree about what Maudebox does,
the implementation has a bug or this specification must be deliberately
changed in the same work.

---

## 1. What Maudebox is

Maudebox is a disposable Docker-based development environment for an existing
host project. It packages a Java and Rust development toolchain, JavaScript
package managers, Git and Jujutsu, browser automation, and the Claude Code,
Codex, and opencode coding agents into one image.

Browser automation includes the Playwright CLI and its MCP server, with the
Chromium revision required by each preinstalled in the image. Configure the MCP
server with `--browser chromium` to select the bundled browser. The CLI and MCP
may require different revisions; both must work without downloading a browser
at runtime when using bundled Chromium.

Its premise is that coding agents should be given a useful development machine
without being given the user's whole machine. A Maudebox container can edit the
selected project, use its real version-control metadata, download dependencies,
and retain agent logins. It does not automatically receive the rest of the
host home directory, host credentials, host agent sessions, or the host Docker
daemon.

Four properties shape the product:

- **The container is the sandbox.** Agent approval loops and nested sandboxes
  are disabled by default where the agent supports it. The project, explicit
  mounts, shared caches, networks, and opt-in devices define the authority the
  container receives.
- **The real worktree remains the worktree.** Maudebox mounts projects at their
  original absolute host paths so Git worktrees and Jujutsu workspaces continue
  to resolve metadata outside the selected directory.
- **Containers are disposable; selected state is durable.** The writable
  container layer disappears on exit. Logins, agent state, explicitly selected
  cache overlays, and Maudebox workspace records have longer, documented
  lifetimes.
- **Sharing is explicit.** Host paths beyond the project and required VCS base
  appear only through built-in narrow mounts or user configuration.

## 2. Vocabulary

### 2.1 Project

A **project** is the canonical host directory launched with Maudebox. It is the
unit shown by `maudebox list` and removed by `maudebox rm`. Several running
instances may use the same project.

### 2.2 Instance

An **instance** is one running Maudebox container. Its handle is the project
basename, optionally followed by the value of `--instance`. The handle is also
the container name, `$MAUDEBOX_INSTANCE`, the terminal title, and, after
sanitization, the container hostname.

Instances end when their container exits. They do not own project state.

### 2.3 Workspace

A **workspace** is a Jujutsu workspace or Git worktree created by `maudebox
new`. Maudebox records enough information to re-enter or remove a workspace it
created. A project supplied directly by path is not owned by Maudebox and is
never deleted by `maudebox rm`.

### 2.4 Persistent state

**Persistent state** is writable tool state kept outside a container's
disposable layer. It includes agent logins and sessions, `gh` authentication,
container-only SSH material, and opencode's XDG state. A **state namespace** is
one independent set of that state, shared by every launch that selects it.

### 2.5 Overlay mount

An **overlay mount** presents a host directory as a read-only starting point
with writes captured in a project-scoped Docker volume. The host directory is
not changed by normal writes through the overlay view.

## 3. Launching a project

`maudebox` launches the current directory. `maudebox <path>` launches another
directory. Arguments after the project select the command to run; with no
command, Maudebox uses `default-command` from the user configuration or opens
an interactive shell.

The selected project is mounted read-write at its canonical absolute host path.
If it is a Git worktree or Jujutsu workspace whose repository metadata lives
elsewhere, the required base repository is mounted at its original absolute
path as well. The container starts in `/root/<project-basename>`, a convenience
symlink to the canonical mount.

The container is removed when the command exits. Files written into the project
or another writable host mount remain on the host. Files written only into the
container layer disappear.

### 3.1 Concurrent instances

Only one running container may have a given instance handle. `--instance NAME`
adds a discriminator so multiple containers can use one project concurrently:

```sh
maudebox --instance review . claude
```

Concurrent instances share project-scoped overlays. They share persistent state
when they select the same state namespace and remain independent when they
select different namespaces. The instance discriminator separates running
containers and their runtime-only sockets; it does not select a login
environment.

### 3.2 Images and commands

`--tag` selects the image tag. Maudebox forwards the command as arguments rather
than evaluating it through a shell, except when expanding a configured alias.
Hyphen-prefixed command arguments are preserved.

Aliases in the user configuration may abbreviate an in-container command and
accept additional trailing arguments. They work in interactive shells and as
the first command in a non-interactive launch.

## 4. The isolation boundary

The container receives:

- the selected project and any required VCS base, read-write;
- the host Maven artifact repository at the dedicated shared-cache path,
  read-write;
- narrow read-only host configuration mounts documented in §7 and §8;
- paths explicitly requested through `--mount` or user configuration;
- Docker networks explicitly selected through `--network` or configuration;
- `/dev/fuse` and broad mount privileges only when FUSE or an overlay mount
  requires them.

It does not receive the host's whole home directory, SSH directory, agent
credential stores, arbitrary environment variables, or Docker engine socket.
An explicit user mount can broaden the boundary and is treated as an intentional
grant.

Maudebox grants `PERFMON` to every container for profiling. It grants
`SYS_ADMIN` and disables the Docker AppArmor profile only when an overlay mount
or `--fuse` requires mounting a filesystem inside the container.

The bundled coding agents are configured to treat the container as their
sandbox. A project or user configuration may tighten an agent's own permissions
where its configuration precedence allows it.

## 5. Workspaces and worktrees

`maudebox new <name>` creates and launches a Jujutsu workspace or Git worktree.
Jujutsu is preferred for colocated repositories. The source defaults to the
current project and may be selected by path or unambiguous project name.

The target defaults to a sibling named `<source-basename>.<name>`. The starting
revision defaults to Jujutsu's `trunk()` or the Git remote default branch.
`--from` selects another revision, and `--fetch` refreshes remote-tracking state
before the revision is resolved.

By default, the workspace and its Maudebox records remain after the container
exits. A later `maudebox <name>` re-enters it when the name resolves uniquely.

### 5.1 Ephemeral workspaces

`maudebox new <name> --ephemeral` removes the workspace, its project-scoped
volumes, and its Maudebox state after the container exits, including after an
error or interrupt. Jujutsu forgets the workspace before its directory is
removed. Git removes the worktree and its branch.

Uncommitted work in an ephemeral workspace is not preserved. Version-control
recovery mechanisms may retain committed changes, but Maudebox does not promise
their retention.

An ephemeral workspace can be kept before exit with `maudebox-keep` inside the
container or `maudebox keep <instance>` on the host. Keeping it removes the
pending cleanup request while retaining the manifest that establishes Maudebox
ownership.

## 6. Host paths and caches

### 6.1 Explicit mounts

`--mount HOST:CONTAINER[:ro|rw|overlay]` adds a host path. A leading `~` is
expanded independently for the host and container. The default mode is `rw`.
Mounts can also be listed in the user configuration; command-line mounts are
added to configured mounts.

Maudebox rejects mount plans whose destinations conflict with built-in mounts
or with each other.

### 6.2 Overlay mounts

Mode `overlay` presents the host source read-only beneath a writable,
project-and-target-scoped upper layer. Changes persist across containers for
that project and target while leaving the host source unchanged.

Different projects receive different upper layers. Concurrent instances of one
project share an upper layer. An absent or empty lower directory causes that
overlay to be skipped with a notice.

Overlay mounts require `SYS_ADMIN` and an unconfined AppArmor profile. Maudebox
adds those privileges only when at least one overlay is active.

### 6.3 Maven repository

The host's `~/.m2/repository` is mounted read-write at the container's Maven
`remote-cache` prefix. Downloaded remote artifacts are therefore shared with
the host and other containers. Resolver locks also live in that shared tree so
writers coordinate.

Artifacts installed from the current build go to a separate `local-installs`
prefix in the container layer and disappear with the container. This prevents
two worktrees from publishing different content to one shared Maven coordinate.
Maven settings and credentials elsewhere under host `~/.m2` are not mounted.

## 7. Persistent identity and state

Each state namespace is one Docker volume holding the following writable state:

| State | Container location | Scope |
|---|---|---|
| Claude | `/root/.claude` | state namespace |
| Codex | `/root/.codex` | state namespace |
| `gh` | `/root/.config/gh` | state namespace |
| SSH | `/root/.ssh` | state namespace, container-only |
| opencode config | `/root/.config/opencode` | state namespace |
| opencode data | `/root/.local/share/opencode` | state namespace |
| opencode state | `/root/.local/state/opencode` | state namespace |
| opencode cache | `/root/.cache/opencode` | state namespace |

The state trees remain separate inside a volume. Removing a namespace forgets
all of them together. Project files, overlay volumes, Maven caches, host
configuration mounts, and generated managed configuration are outside state
namespaces.

### 7.1 Credentials

Agent, `gh`, and SSH credentials created inside a container stay in the state
volume. Maudebox does not import corresponding host credential stores. A login
performed in one container is therefore available in future containers using
the same namespace, while remaining independent of the host and other
namespaces.

Credentials in the state volume are reconstructible state: removing the volume
requires logging in again.

### 7.2 Sessions and databases

Codex and opencode keep session databases on the named volume. Concurrent
processes on the same host may share them using their database locking and WAL
behavior. These databases are not placed on a host bind mount or overlay upper
directory.

Application runtime sockets and locks whose lifetime is one container are kept
outside shared persistent state. In particular, Codex's app-server control
directory is isolated per container.

### 7.3 SSH

`/root/.ssh` starts empty and is never a mount of host `~/.ssh`. Keys,
`known_hosts`, and configuration created there are shared by Maudebox containers
using the same namespace. Its permissions are repaired at launch so OpenSSH
accepts it.

### 7.4 State namespaces

`--state NAME` selects an independent state namespace for a launch:

```sh
maudebox --state work .
maudebox --state personal new feature-x
```

The `state` user-config key supplies the selection when the flag is absent. The
flag takes precedence over configuration; without either, Maudebox uses the
`default` namespace. `default` is reserved for the original `maudebox-state`
volume, so existing installations retain their logins without migration.

Other namespace names contain 1–32 lowercase ASCII letters, digits, or single
hyphens and start and end with a letter or digit. A named namespace maps
one-to-one to a labeled volume named `maudebox-state-<name>`. Invalid names fail
before Docker is invoked. A launch creates its selected volume when needed.

The namespace applies as a unit to every row in the state table above. It is
available as `$MAUDEBOX_STATE` for diagnostics, but bundled tools do not inspect
that variable. `maudebox list` reports the namespace beside every running
instance. `--instance` remains an independent container handle and does not
change state selection.

`maudebox state list` lists discovered namespaces and their running instances.
`maudebox state rm NAME` removes the namespace volume and reports that its
credentials, sessions, and tool state will be forgotten. Removal is refused
while any running or stopped container uses the volume. Project removal and
ephemeral workspace cleanup never remove state namespaces.

## 8. Agent configuration

Maudebox bundles Claude Code, Codex, and opencode at image-pinned versions.
Image rebuilds control upgrades; bundled agents do not silently replace their
installed versions.

Project-level instruction files come from the mounted project. Selected global
instructions, skills, commands, plugins, and settings are mounted read-only
from the host. Credential-bearing configuration and host-path-keyed session
state are excluded.

### 8.1 Shared MCP servers

`[mcp.NAME]` tables in the Maudebox user configuration declare MCP servers for
all three agents. Maudebox translates the shared declaration into each agent's
configuration dialect. `[mcp.NAME.codex]` and `[mcp.NAME.opencode]` subtables
carry agent-specific keys and are hidden from the other agents.

Managed MCP changes apply to newly launched containers. A running container
keeps the generated configuration snapshot with which it started.

### 8.2 Codex settings and remote control

The `[codex]` table passes host-sourced Codex settings into every container at
managed precedence. MCP servers are excluded from this table because §8.1 is
their common source.

Codex remote control uses outbound connectivity and the selected Codex login.
Pairing persists in the selected state namespace. The instance handle becomes the
container hostname and therefore the device name. Its control socket remains
per-container so one instance cannot attach to another instance's local daemon.

### 8.3 opencode server surfaces

Maudebox does not publish ports automatically. An opencode server is reachable
from the host only where the host container runtime routes container addresses,
and from other containers only on a shared Docker network. Exposure to another
machine requires networking outside Maudebox's current interface.

## 9. Networking and FUSE

Every container resolves `host.docker.internal` to the host gateway. Services
published on the host can be reached through that name.

`--network NAME` joins an existing Docker network and is repeatable. When any
network flags are given they replace the configured network list; otherwise
the configuration is used. Maudebox does not create the named networks.

`--fuse` exposes `/dev/fuse` and grants the capabilities needed to mount a FUSE
filesystem. FUSE clients are installed by the user as needed. A FUSE mount is
visible only inside that container and ends with it. Launch fails if the host
does not provide `/dev/fuse`.

## 10. Configuration

The user configuration is TOML at
`${XDG_CONFIG_HOME:-~/.config}/maudebox/config.toml`. It may define:

- project roots used to resolve bare project names;
- a default in-container command;
- explicit mounts and overlay mounts;
- command aliases;
- one or more Docker networks;
- default FUSE access;
- a default state namespace;
- shared MCP servers and agent-specific MCP overrides;
- managed Codex settings.

Configuration values are type-checked when read. Invalid configuration stops
the launch with an error naming the key. `maudebox config path` prints the
location, and `maudebox config edit` opens it using `$VISUAL`, `$EDITOR`, or
`vi`.

## 11. Listing and cleanup

`maudebox list` shows one row per project that is running, owns a project
volume, or has a Maudebox workspace manifest. Running instances are listed
within the project row. An instance ceases to exist when its container exits.

`maudebox rm <project>` removes Maudebox-owned state for exactly one project:
its overlay volumes and state directory. If Maudebox created the workspace, it
also removes that workspace or worktree. A directly supplied project directory
is left intact.

Removal is refused while any instance of the project is running. State
namespaces are outside project cleanup and are managed with `maudebox state`.

## 12. Platform behavior

On macOS container runtimes that translate host file ownership, the container
runs as root. On Linux, privileged setup occurs as root and the entrypoint then
drops to the invoking host UID and GID so files written to host mounts have the
expected ownership. Passwordless `sudo` remains available inside the container.

The image supports x86-64 and ARM64. A new bundled tool is part of the product
only when the image can obtain or build it for both supported architectures.

Terminal capability and identity variables are forwarded. The terminal or tmux
window is named after the instance for the duration of the launch.

## 13. Deliberate limits

Maudebox currently does not:

- expose the host Docker daemon or provide Docker-in-Docker;
- publish arbitrary container ports;
- create Docker networks;
- preserve arbitrary changes made only in the container layer;
- import host agent, `gh`, or SSH credentials;
- maintain more than one persistent identity or state namespace;
- protect explicitly mounted host paths from a process that has been granted
  write access to them.

These are product boundaries, not accidental omissions. Expanding one requires
updating this specification and, when the choice has durable consequences, a
decision record.

## 14. Design principles

1. **The container is the authority boundary.** Agent defaults should use it
   instead of layering prompts that obscure what the container can actually do.
2. **A mount is a grant.** Every host path, device, network, and capability has
   an explicit reason to cross the boundary.
3. **Match state lifetime to its owner.** Project state belongs to projects,
   launch state to containers, and global identity state to its declared global
   store.
4. **Preserve host path identity.** Version-control metadata and build tools
   should see the same absolute project paths inside and outside the container.
5. **Keep reconstructible credentials separate from authored configuration.**
   Writable credentials stay inside Maudebox; durable instructions and skills
   may enter as narrow read-only mounts.
6. **Make cleanup ownership explicit.** Maudebox removes worktrees it created
   and never infers ownership merely from having launched a directory.
7. **Prefer one shared declaration over agent-specific duplication.** Common
   facilities such as MCP servers are translated at the boundary, with narrow
   escape hatches for real differences.
