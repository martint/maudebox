# CLAUDE.md

Guidance for Claude Code (claude.ai/code) when working in this repository.

`docs/SPEC.md` is the source of truth for user-visible behavior and deliberate
limits; `docs/decisions/` holds numbered decision records; `README.md` is the
public operating guide. This file holds the implementation constraints and the
reasons behind them — the things that are not derivable from the code and that
have already been gotten wrong once.

## What this repo is

A Docker dev environment shipping JDK 26 (Temurin), `mvnd`, a rustup-managed
Rust toolchain, `bun`, `pnpm`, `git`, `jj`, Playwright, and the Claude Code,
Codex and opencode CLIs in one image. It runs against an arbitrary host project
directory, bind-mounted at its original host path inside the container so
jj/git worktree metadata resolves.

There is no application source here — changes to this repo are changes to the
dev-container itself. It is a Cargo workspace building two things:

- the host-side `maudebox` wrapper — root package (`Cargo.toml`, `src/`);
- the image — `docker/` (`Dockerfile`, `entrypoint.sh`, `prompt.sh`,
  `aliases.sh`, `maudebox-keep`), driven by the `xtask` crate wired as a cargo
  subcommand via `.cargo/config.toml`.

## Common commands

```
cargo build --release     # wrapper → target/release/maudebox
cargo xtask image         # docker image
cargo xtask all           # both
cargo test                # wrapper unit tests
```

Pinned tool versions live in one place: the `DEFAULT_*_VERSION` constants in
`xtask/src/main.rs`, each overridable with a `--<tool>-version` flag. Don't
duplicate the values here; `README.md` quotes a couple for the pitch and must
stay in sync with those constants.

To validate a change end-to-end, rebuild the image and exercise the affected
path with `maudebox`.

## Architecture

### Maven split local repository

Every launch bind-mounts the host repository RW at
`/root/.m2/repository/remote-cache`; `local-installs` stays in the container's
writable layer and dies with `docker run --rm`. The Dockerfile enables
Resolver's enhanced split repository (`remotePrefix=remote-cache`,
`localPrefix=local-installs`).

`MAVEN_OPTS` covers conventional Maven and the Maven Wrapper.
`/usr/local/bin/mvnd` is a launcher that prepends the same values as Maven
**user properties** to every request before delegating to `/opt/mvnd/bin/mvnd`
(`mvn` points at the launcher too). Request properties survive a project's
`.mvn/mvnd.properties` and `.mvn/jvm.config`; daemon JVM properties do not,
because a project's `mvnd.jvmArgs` can replace them wholesale.

`aether.syncContext.named.basedir.locksDir` points at `remote-cache/.locks`, so
every container and the host Maven share one conventional lock directory.

There is no Maven volume to label, list, chown or remove. An explicit overlay
encompassing `/root/.m2/repository` is rejected — it would collide with the
built-in bind. This design intentionally exposes the host artifact cache RW:
Resolver routes normal installs away from it, but any container process can
still mutate it. Settings and credentials elsewhere in `~/.m2` stay unmounted.

### Opt-in three-layer cache via overlayfs

The non-obvious piece is in `entrypoint.sh`. For each mount spec using mode
`overlay`, the wrapper bind-mounts the host source read-only at
`/maudebox/overlay-N/lower`, creates a per-worktree+per-target volume
(`maudebox-overlay-<basename>-<projhash>-<targethash>`) at
`/maudebox/overlay-N/upper`, and appends a `lower:upper:target` triple to
`MAUDEBOX_OVERLAYS`. The entrypoint loops over that var and mounts an `overlay`
fs at each target with `lowerdir=<lower>`, `upperdir=<upper>/upper`,
`workdir=<upper>/work`.

Both upperdir and workdir must live inside the per-overlay volume (host-fs
backed), **not** on the container rootfs: kernel overlayfs refuses to use
another overlay as upperdir/workdir, and Docker's rootfs is itself overlayfs. A
named volume sidesteps that.

Mounting overlayfs from inside a container needs `--cap-add SYS_ADMIN` and
`--security-opt apparmor=unconfined`, supplied only when an overlay was
requested. An empty or absent lowerdir logs a notice and skips that overlay
rather than failing. With no overlay specs the container gets neither
privilege, `MAUDEBOX_OVERLAYS` is unset, and the entrypoint loop is a no-op.
Independently, every launch adds the narrower `PERFMON` capability so Docker's
capability-aware seccomp profile admits `perf_event_open(2)` for the bundled
`perf`.

### FUSE (opt-in)

`--fuse` adds `--device /dev/fuse` plus the same `SYS_ADMIN` +
`apparmor=unconfined` pair; `runtime_privilege_args` emits that pair once when
either feature asks for it. Both are load-bearing even though `fusermount3` is
setuid root: a setuid binary only regains capabilities present in the
container's bounding set, Docker's default seccomp admits `mount(2)` only for
`SYS_ADMIN`-capable containers, and the `docker-default` AppArmor profile
denies mounts outright.

The device node is separate from the capability because the FUSE *client* — not
the helper — opens `/dev/fuse`. The entrypoint relaxes its mode to 0666 while
still root, since Docker reproduces the host node's ownership and a
`root:root 0660` host node would be unopenable after the privilege drop.

The image installs `fuse3` (helper, plus `user_allow_other` in `/etc/fuse.conf`
so a mount made by the dropped-privilege user is visible to root) but no FUSE
clients — those are `sudo apt-get install` inside the container.

### Container runs as root, then drops privileges on Linux

The container starts as root and stays that way for privileged setup — the
overlayfs mount needs `CAP_SYS_ADMIN` and volume mounts are root-owned at first
creation. Then behavior diverges by host platform.

**macOS / OrbStack: stays root for the whole session.** OrbStack root-squashes
virtiofs bind-mounts, so lowerdir and project files appear as `uid=0` inside
the container and root's writes translate back to the host user. Anything else
breaks overlayfs copy-up: copy-up preserves lowerdir UID, so a `uid=0` lowerdir
yields a root-owned upperdir a dropped-privilege user can't write. This was
tried once with `ubuntu`/UID 1000 + `runuser` — mvnd registry, Aether lock
files and install-plugin tmp files all broke, each needing its own workaround.

**Linux Docker: drops to the host UID/GID before `exec`.** Linux bind mounts
pass UIDs through literally — no root-squash — so a root-in-container write
into the project would land root-owned on the host. The wrapper passes
`HOST_UID`/`HOST_GID`; the entrypoint rewrites `/etc/passwd` so that UID's home
is `/root` (Java's `user.home` reads pw_dir, not `$HOME`), `chown`s the
writable volume trees (every `/maudebox/overlay-*/upper`, `/root/.claude`,
`/root/.config/gh`, `/root/.ssh`, `/root` itself) using `find -xdev ! -uid
$HOST_UID` so it doesn't touch bind mounts on another fs — and only when the
tree's root isn't already host-owned. Everything the dropped-privilege user
writes lands host-owned, so deep recursion only does real work on first launch
against an empty volume. Then `exec setpriv --reuid=… --regid=…
--clear-groups -- "$@"`. Copy-up is fine here because the host source is
already host-uid-owned.

Everything lives at `/root` on both platforms: `/root/.claude`, the Maven
mounts, overlay targets, and a `/root/<basename>` symlink to the worktree's
host path. The `ubuntu` user from the noble base may collide with
`HOST_UID=1000`, which is why the entrypoint **replaces** any existing passwd
entry for `HOST_UID` rather than appending — otherwise `/home/ubuntu` would win
as pw_dir.

Because of the privilege drop, in-container privileged operations fail with
`Permission denied`. The image installs `sudo` with a `/etc/sudoers.d/host`
rule granting the runtime `host` user passwordless sudo, so the escape hatch is
`sudo apt-get install …` (or `sudo -i`). The rule is inert on macOS/OrbStack.

### jj workspaces / git worktrees

Both store an absolute (or cwd-relative) path to the *base* repo inside the
worktree's metadata: `<workspace>/.jj/repo` is a file containing the path to the
main repo's `.jj/repo`; `<worktree>/.git` is a file containing
`gitdir: <abs path>`. Mounting the worktree at an unrelated container path
leaves those references dangling and jj/git fail confusingly.

So: `detect_vcs_base()` reads `.jj/repo` or `.git`, resolves any relative
component, and prints the base repo's working-tree path. The project and base
repo are both bind-mounted at their **host paths**. `HOST_PROJECT_DIR` is
passed as an env var; the entrypoint creates a `/root/<basename> → <host
worktree path>` symlink and `cd`s there before `exec`. The shell sits at a
short friendly path while the filesystem underneath is the host-path bind, so
metadata resolves.

A regular `git` checkout (`.git` is a directory) or a non-VCS dir makes
detection return empty, and only the project itself is mounted.

### Shell prompt (jj/git aware)

`prompt.sh` is `COPY`'d to `/etc/profile.d/dev-prompt.sh` and sourced from
`/etc/bash.bashrc`. It defines `__dev_prompt_vcs` as `PROMPT_COMMAND` and a
two-line `PS1`: `cyan/path  yellow/(vcs)\n green/>`. Bash convention is `#` for
root and `$` for everyone else, but on Linux the session *was* root and then
dropped, so neither matches cleanly — `>` sidesteps it.

VCS info tries jj first, falling back to git's symbolic-ref/short-hash. The
`closest_bookmark(to) = heads(::to & bookmarks())` revset alias is passed inline
via `jj log --config=…` so the user's jj config needn't be mounted.

### Terminal identity forwarding

A TUI inside the container drives whatever terminal is on the other end of the
tty, so the wrapper forwards `TERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`,
`COLORTERM` and `LC_TERMINAL*`.

`TERM_PROGRAM` selects an escape *dialect*. opencode wraps clipboard writes in
tmux's DCS passthrough (`\ePtmux;…`) when it sees `tmux`, and only something
that unwraps DCS acts on it — claiming tmux off-tmux silently breaks the
clipboard, and not claiming it inside tmux breaks it the other way. tmux 3.2+
exports the value itself; the wrapper forces it when `$TMUX` is set so older
tmux is covered.

`TERM` selects *capabilities*. `docker run -it` defaults it to a bare `xterm`
when absent, advertising a 1984 terminal to everything that consults terminfo.
Forwarding the host value is the fix; the image installs `ncurses-term` for the
extended database, and the entrypoint validates the forwarded value with
`infocmp`, falling back to `xterm-256color` with a notice — ncurses programs
abort on an unknown `TERM` rather than degrading, so a bad forward is worse
than none.

Note what this does *not* buy: enhanced key reporting is negotiated at runtime,
not derived from `TERM`. opencode, measured under a pty, sends the same probes
(`CSI ? u`, `CSI > 4 ; 1 m`) under `xterm`, `xterm-256color`, `screen-256color`
and `tmux-256color` alike, and honors legacy control bytes regardless. Don't
reach for `TERM` when diagnosing a key that doesn't register.

Two capabilities stay out of reach from inside a container: naming the tmux
window (done host-side) and writing the system clipboard, which has no route
out except OSC 52 and so depends on tmux's `set-clipboard`/`allow-passthrough`
and the terminal's own permission setting.

### Host git/jj config sharing

Mount list and rationale: SPEC §6.4. The constraint worth repeating here is why
jj gets `~/.config/jj/config.toml` **only** and not the whole `~/.config/jj/`:
`repos/` underneath is jj's per-repo state cache keyed by host paths.

Signing is forced off via Dockerfile `GIT_CONFIG_COUNT/KEY/VALUE`
(`commit.gpgsign=false`, `tag.gpgSign=false`) — env vars because they override
*mounted user config*, same precedence as `git -c`. The motivating case is the
host's `commit.gpgsign = true` paired with 1Password's macOS-only ssh-sign
program, which would auto-fail every container commit.

The container's `~/.ssh` is a maudebox-managed state subtree that starts empty
and is never populated from the host.

### Persistent state (Claude + Codex + opencode + gh + ssh)

One state namespace volume per launch (see SPEC §7 for selection rules and the
subtree table). Each subtree is mounted at the canonical path its tool expects
via Docker's `volume-subpath`, so the trees never mix on disk despite sharing a
volume. `volume-subpath` fails if the subdir doesn't exist, so the wrapper runs
a throwaway `mkdir -p` container over the whole list before each launch
(idempotent, ~50ms). Requires Docker 25+.

Namespace names use a reversible lowercase slug so the user-visible name maps
one-to-one to the volume name. Named volumes carry `maudebox.kind=state` and
`maudebox.state=<name>`; the legacy `default` may remain unlabeled. A named
volume that exists *without* the expected label is rejected rather than
silently consumed. `state rm` checks running **and stopped** containers before
removal. Project and ephemeral cleanup never own a state namespace.

**SQLite on the named volume is the load-bearing constraint.** Codex's
`$CODEX_HOME` state DB and opencode's session DB both run WAL with a 5s
busy_timeout. Sharing one DB across concurrent containers is safe for the same
reason it is across concurrent sessions on a normal host: WAL needs all writers
on one host against a filesystem with working `fcntl` locking + mmap, and a
Docker **named volume** lives in the single Docker/OrbStack VM's own ext4 (host
bind mounts use virtiofs; named volumes do not). So every container hits the DB
as processes on one VM kernel against one real local fs. Never relocate either
DB onto a host bind or an overlay upperdir — `OPENCODE_DB` and `CODEX_HOME`
would both let you, and WAL genuinely breaks there.

opencode gets four subtrees rather than one because it follows the XDG split
instead of a single home dir (`opencode debug paths` prints the resolved set).
Its `state/` locks are keyed to the data directory, not the project, which is
exactly what serializes access to that shared DB — so they belong on the volume
beside it, not in the container layer.

Codex keeps its login token inside `$CODEX_HOME`, so no symlink redirect is
needed (unlike Claude's). On Linux the entrypoint `chown`s `codex/` alongside
`claude/`/`gh/`/`ssh/`.

The `ssh/` subtree gives every launch in a namespace a shared, container-only
`~/.ssh`. Docker creates a `volume-subpath` dir mode 0755, but OpenSSH wants
0700 (and refuses keys under StrictModes otherwise), so the entrypoint
`chmod 700`s it every launch while still root, and `chown`s it on Linux.

`~/.claude.json` (login token + project list) lives outside `~/.claude/` on the
host and so can't ride the volume mount; the entrypoint symlinks
`~/.claude.json → ~/.claude/state.json` so writes follow into the volume. One
`claude` login per namespace; same for `gh auth login`.

#### Host config layered on top

Read-only, per file or per subdirectory — never a whole agent home dir, which
would drag in host-path-keyed state:

- **Claude** `~/.claude/`: `CLAUDE.md`, `settings.json`, `agents/`, `commands/`,
  `skills/`, `plugins/`. Excludes `projects/`, `todos/`, `statsig/`,
  `shell-snapshots/`.
- **Codex** (only when present): `~/.codex/AGENTS.md` → `/root/.codex/AGENTS.md`
  and `~/.agents/skills` → `/root/.agents/skills`. `AGENTS.md` is Codex's
  `CLAUDE.md`-equivalent *and* its memory analog — Codex has no separate
  auto-memory store, so there is no `--memory-from` counterpart. Project-level
  `AGENTS.md` needs no handling; it's already inside the mounted worktree.
- **opencode** `~/.config/opencode/`: `AGENTS.md`, `tui.json`, `agent/`,
  `command/`, `skill/`, `plugin/`. `opencode.json` is deliberately excluded —
  it's where `provider.<id>.options.apiKey` lives, and binding it would import
  host credentials and defeat the credential separation; the user can still
  mount it via a `mounts` entry. opencode needs no instruction mount of its own:
  it falls back to `~/.claude/CLAUDE.md` when `~/.config/opencode/AGENTS.md` is
  absent and auto-loads skills from `~/.claude/skills/` and `~/.agents/skills/`,
  all already bound.

No agent's credential store is bind-mounted from the host — that's the point:
container credentials stay separate from the host user's, so a container can
hold narrower ones. The cost is one login if the state volume is dropped, which
is cheap and rare. Host-side mounts are limited to hand-authored config, which
is the part that *isn't* reconstructible.

One narrow carve-out under Claude's `projects/`: the auto-memory directory
`~/.claude/projects/<encoded-cwd>/memory/` is bind-mounted **read-write**
(behavior in SPEC §8.4). The encoding maps `/` and `.` in the canonical cwd to
`-`, and since the project is mounted at its host path, host and container
normally agree on the key. The rest of `projects/<key>/` stays in the volume.
`--memory-from` overrides only the *host* side of the bind — the container
target stays keyed to the project's cwd, because that's what Claude Code looks
up inside.

#### opencode permission seed

The entrypoint seeds `{"permission": "allow"}` into
`/root/.config/opencode/opencode.json` when absent — opencode's analog of
`IS_SANDBOX=1` and Codex's `sandbox_mode = "danger-full-access"`, since the
container is already the sandbox. It goes in the **global** layer, not the
generated managed one, precisely because managed config outranks project
config: a per-project `opencode.json` must stay able to tighten it. It's
written as root before the privilege drop and chowned explicitly, like
`~/.claude/state.json`, since the chown sweep skips a tree whose root is
already host-owned.

### Managed MCP and agent config generation

One set of `[mcp.NAME]` tables drives **all three** agents. Generated files are
bind-mounted, not routed through the state volume; all mount-target parent dirs
are created in the Dockerfile so they always exist. **Atomic rename matters**:
Docker captures the file's inode at mount time, so a later rewrite from a
concurrent launch with different config doesn't perturb a running container.

| Agent | Generated at `$XDG_STATE_HOME/maudebox/` | Mounted at | Layer | Written |
|---|---|---|---|---|
| Claude | `managed-mcp.json` | `/etc/claude-code/managed-mcp.json` | enterprise-managed | only with ≥1 `[mcp.*]` |
| Codex | `codex-config.toml` | `/etc/codex/config.toml` | system (lowest) | **every launch** |
| Codex | `codex-managed-config.toml` | `/etc/codex/managed_config.toml` | enterprise-managed | only with `[codex]` |
| opencode | `opencode-config.json` | `/etc/opencode/opencode.json` | managed (highest) | only with ≥1 `[mcp.*]` |

Note the inversion: opencode's managed layer is the *highest* precedence (like
Claude's managed scope, unlike Codex's system layer), so it carries MCP servers
only — anything a user might reasonably vary per project must go somewhere
overridable, which is why the permission seed lives in the global config.

**Codex is a real translation, not a pass-through.** It uses
`[mcp_servers.NAME]` with no `type` (transport inferred from `command` vs
`url`), `http_headers` instead of `headers`, and `additionalProperties: false`,
so unknown fields are rejected. `translate_mcp_for_codex` maps the shared
fields (`command`/`args`/`env`/`url`/`cwd`, `headers` → `http_headers`), drops
`type`, and warns-and-drops anything else.

**opencode's dialect** (`translate_mcp_for_opencode`): transport is an explicit
`"type": "local" | "remote"` chosen by whether `url` is present; a local server
takes program and arguments as one `command` array with `env` renamed to
`environment`; a remote one takes `url`/`headers`. Claude's `type` hint is
dropped, and fields that don't apply to the chosen transport are dropped with a
warning since opencode rejects unknown keys.

Per-server `[mcp.NAME.codex]` and `[mcp.NAME.opencode]` sub-tables are the
escape hatches for agent-only keys (`bearer_token_env_var`,
`startup_timeout_sec`, `enabled`, `timeout`, `oauth`, …): contents pass through
verbatim and merge last so they override translated fields. Each agent's
reserved sub-table is stripped from the other two agents' configs.

The Codex **system** `config.toml` is written on every launch even with no
`[mcp.*]` tables because it also carries the sandbox default
(`sandbox_mode = "danger-full-access"`, `approval_policy = "never"`) and
`check_for_update_on_startup = false`. Being the lowest layer, a user's own
`~/.codex/config.toml` in the state volume still overrides it — which is the
point.

A `[codex]` table is the route for host-sourced Codex settings (model provider,
reasoning effort, …), necessary because the wrapper forwards no arbitrary env
vars and `$CODEX_HOME` is never seeded from the host, so anything set
in-container dies with the state volume. It goes to the **enterprise-managed**
layer rather than being appended to the system config because Codex silently
drops `model_provider`/`model_providers` from the system layer, and from the
project layer too (there with a warning). Managed is the lowest layer that
honors them, and it outranks `~/.codex/config.toml` *and* `-c`, so those keys
are pinned — which is exactly why maudebox's own sandbox defaults stay in the
system config, where they remain overridable. `mcp_servers` is rejected in
`[codex]`; it belongs in `[mcp.NAME]`, which feeds all three agents.

### Container identity, labels, and the `keep` flag

Every container is named after its instance (the same string
`$MAUDEBOX_INSTANCE` carries inside, via `docker run --name`) so `docker ps` /
`exec` / `logs` take a stable human handle instead of `clever_curie`. The
**hostname** is set to the same handle (`--hostname`, sanitized to a valid RFC
1123 label by `sanitize_hostname`) because Codex's remote-control identifies a
paired server by hostname with no config override — without this, each worktree
would enroll in the ChatGPT device list under Docker's random container-ID
hostname. This is the Codex analog of what `$MAUDEBOX_INSTANCE` does for
`claude --remote-control`.

The wrapper still never depends on the name for lookup — that's what labels are
for, and **the label taxonomy follows lifetime, which is load-bearing**:

- **Containers**: `maudebox.instance`, `maudebox.project`, `maudebox.state`,
  `maudebox.ephemeral`, plus `maudebox.ephemeral-name` and
  `maudebox.state-dir` for ephemeral runs. All selections or properties of one
  launch, living exactly as long as the launch. The state name is also exposed
  as `$MAUDEBOX_STATE` for diagnostics.
- **Project volumes**: `maudebox.project`, and `maudebox.overlay-target` for
  overlays.
- **State volumes**: `maudebox.kind=state` plus the namespace name.

Don't put a launch property on a project volume. Overlay volumes were once
created with the launch labels copied onto them; since `docker volume create`
silently ignores labels on an existing volume, the result permanently recorded
whichever launch created it first, which `list` then read back as though that
instance owned the project. Relatedly, `list` rows were once keyed on
`(project, instance)` with the instance read from a volume label — so a stopped
project displayed one arbitrary instance as its owner while its other instances
were invisible. Instance is a container-lifetime concept; don't reintroduce it
as a property of anything on disk. A name collision (two concurrent containers
on one project without `--instance`) surfaces as a `docker run` error.

Codex's fixed managed-daemon directory `/root/.codex/app-server-control` is a
**tmpfs** mount. It must be isolated: the path sits inside the `$CODEX_HOME`
subtree every container shares, it holds a startup lock and a Unix socket, and
a socket on a shared volume is connectable across containers — so a second
daemon would either refuse to start or hand a remote-control frontend another
worktree's session. tmpfs is the right instrument because the isolation is
per-container while the contents outlive nothing. A named volume was used here
once, which turned an application runtime detail into a maudebox-managed object
with a name, labels and a lifetime, and left `list` and `rm` reasoning about a
Codex artifact. Keep application scratch state out of the wrapper's volume
namespace.

The per-instance state dir is
`${XDG_STATE_HOME:-~/.local/state}/maudebox/instances/<volume-name>/` (keyed
off the same volume name as the overlay). `maudebox new` always creates it and
writes `manifest.toml` recording workspace kind, source repo and `<name>` — its
presence is what tells `maudebox rm` that maudebox created the worktree, versus
a path handed to `maudebox <path>` directly. For ephemeral runs the dir is also
bind-mounted at `/run/maudebox/` so the in-container `maudebox-keep` script and
the host-side `maudebox keep <name>` can drop a `keep` file. On exit, ephemeral
cleanup reads `<state-dir>/keep`: if present, only the keep flag is removed
(the manifest stays, so a future `rm` still recognizes maudebox ownership);
otherwise `rm` does the full teardown.

`cmd::new` calls `cmd::run::run` in-process when launching the inner container,
so ephemeral state (name, source, kind) never travels through CLI args — it's
held in the call stack and persisted via the manifest. There is deliberately no
public flag for "launch this inner container as ephemeral"; the way to spawn
ephemerals is `maudebox new --ephemeral`.

Overlay volume names combine a human-readable project key with a target hash:
the basename keeps them recognizable, the hash prevents collisions when two
worktrees share a basename.

### Multi-arch build

The `mvnd`, `jj`, Rust, `bun` and `pnpm` install steps branch on `uname -m` to
pick `amd64`/`aarch64` artifacts, so the image builds natively on Apple Silicon
and x86_64. Per-tool asset choices:

- `jj` — musl static binaries, avoiding libc-version coupling to the base image.
  Its release tarball stores members with a `./` prefix, so `tar -x … ./jj` is
  required (not `… jj`).
- Rust — gnu triple, since the toolchain dynamically links the same glibc the
  base image provides. Installed via pinned `rustup-init` with
  `RUSTUP_HOME=/usr/local/rustup` and `CARGO_HOME=/usr/local/cargo`; keeping
  both under `/usr/local` holds the toolchain out of any `~/.cargo` overlay, and
  `chmod -R a+w` on the install dirs lets the Linux dropped-privilege user
  populate the registry cache without sudo. For per-worktree isolation of
  cargo's package cache, add an overlay spec for `/usr/local/cargo` (or point
  `CARGO_HOME` under `~`) — the toolchain stays put either way.
- `bun` — glibc build, not `-musl`, matching the base.
- `pnpm` — ships a native launcher plus a `dist/` JS tree that must stay
  co-located, so the whole tarball is extracted under `/opt/pnpm` with the
  launcher symlinked onto `PATH`.

Global installs land in the runtime user's HOME: `bun install -g` at
`~/.bun/bin`, `pnpm install -g` in `$PNPM_HOME` (preset to
`~/.local/share/pnpm`). Both are prepended to `PATH` in the Dockerfile so `-g`
installs are discoverable and `pnpm setup` needn't run in every container —
pnpm refuses global installs without `PNPM_HOME`. An overlay spec for either
dir is supported but **not** motivated by Maven's clobber-by-coordinate
problem: bun/pnpm/cargo caches are content-addressed (keyed by tarball hash or
`name-version`) and workspace-internal deps resolve via path/workspace links,
never through the cache, so concurrent worktrees sharing one cache are safe at
the artifact level. Overlay still buys hermeticity, race-free cold-fill under
heavy concurrency, and isolation of the global-bin namespace.

### Claude Code install

Installed as the native binary via `curl … | bash -s -- "$CLAUDE_VERSION"` (the
install script takes a `stable | latest | X.Y.Z` positional) and moved to
`/usr/local/bin/claude`. The installer drops a launcher symlink at
`$HOME/.local/bin/claude` whose target lives in
`$HOME/.local/share/claude/versions/<ver>`; the Dockerfile resolves it with
`readlink -f`, moves the real binary into `/usr/local/bin`, then removes
`$HOME/.local/share/claude` and `$HOME/.claude` so the install dir doesn't
bloat the image and `$HOME/.claude` is empty for the runtime volume mount.
After the move, `$HOME/.local/bin/claude` is recreated as a symlink to
`/usr/local/bin/claude`: the native binary records `installMethod=native`
(persisted into the volume's `state.json`) and warns at startup if that symlink
is missing.

`DISABLE_AUTOUPDATER=1` is set because that same `installMethod=native` stamp
comes paired with `autoUpdatesProtectedForNative=true` in the shared
`state.json`, and that pair makes Claude Code *ignore* a configured
`autoUpdates: false` — only the env var stops the updater. Left running, it
downloads a newer version into the ephemeral container layer and re-points
`~/.local/bin/claude` (first on `PATH`) at it, which both un-pins
`CLAUDE_VERSION` and leaves `claude` failing with `Permission denied` while the
staged binary is still non-executable. This is the Claude-side counterpart of
`check_for_update_on_startup = false` in the generated Codex config.

### Codex install

Codex is installed as the installer-managed **"standalone" layout**, not a bare
binary — load-bearing for remote control. `codex remote-control start` (the
daemon the ChatGPT app and a local `codex --remote` TUI attach to) refuses to
run unless it finds the standalone install at
`$CODEX_HOME/packages/standalone/current/codex`, because it starts and updates
the app-server from that fixed path; a plain binary on `PATH` fails with
"managed standalone Codex install not found".

So the Dockerfile pulls the release **package** bundle
(`codex-package-<triple>.tar.gz`, which also vendors the `bwrap` sandbox helper
and `rg` that Codex expects) rather than the single-binary asset, and
reproduces the installer's layout: `packages/standalone/releases/<ver>-<target>/`
(extracted bundle plus a `codex → bin/codex` symlink) with `current` → that
release. The release tag is `rust-v<ver>`.

The non-obvious part is *where* it lives. `$CODEX_HOME` is `/root/.codex`, a
state-volume subpath that is **empty at runtime** (the mount shadows anything
baked at build time), yet the daemon needs the standalone install under
`$CODEX_HOME`. So the layout is staged at **`/opt/codex`** (outside the volume,
always present, `chmod a+rX`), and the entrypoint symlinks the volume-backed
`$CODEX_HOME/packages/standalone/current` → `/opt/codex/packages/standalone/current`
on first launch — a symlink, not a ~336 MB copy into the volume (the `codex`
binary alone is ~285 MB). The daemon accepts a `current` resolving to an
external release dir. `releases/` is left as a real dir in the volume so a
daemon-side update writes there rather than failing against read-only `/opt`;
`check_for_update_on_startup = false` keeps that from silently un-pinning the
version. Pointing `current` at `/opt`'s own version-agnostic `current` means an
image rebuild with a new `CODEX_VERSION` is picked up with no volume change.

`/usr/local/bin/codex` symlinks the same standalone binary. `ENV
CODEX_HOME=/root/.codex` is set and the dir pre-created — Codex hard-errors if
`CODEX_HOME` points at a missing path.

### opencode install

The simplest of the three: a single bun-compiled binary (~180 MB) in a per-arch
tarball, extracted straight to `/usr/local/bin/opencode` and verified against
the release asset's recorded digest via the GitHub API — the jj/pnpm pattern,
not Claude's installer script or Codex's layout reproduction. Upstream is
`anomalyco/opencode` (moved from `sst/opencode`) and ships several releases a
day, so the pin drifts fast.

Two asset choices are deliberate. The **glibc** (not `-musl`) build matches the
noble base, like bun. The **non-baseline** x64 build requires AVX2:
`-baseline` exists for older CPUs, and the failure mode on one is SIGILL at
first invocation, not a graceful error. The installer script picks this by
reading `/proc/cpuinfo`, but under `docker build` that reports the build host,
so it is pinned here instead.

`OPENCODE_DISABLE_AUTOUPDATE=true` is set for the same reason as Claude's
`DISABLE_AUTOUPDATER`: an update would un-pin `OPENCODE_VERSION`, and would
fail partway besides, since the binary sits in root-owned `/usr/local/bin`
while the session runs as the host UID. The config-file equivalent
(`autoupdate: false`) is only honored at the global config path, so the env var
is the reliable lever.

opencode gets no remote-control equivalent. It exposes `opencode serve`/`web`
(a listening HTTP server with optional basic auth) rather than Codex's outbound
websocket, and the wrapper publishes no ports, so reaching it off-host would
need a `docker run -p` flag maudebox doesn't have. `docker exec -it <instance>
opencode` covers the second-terminal case.

### Adding another agent CLI to the image

Claude Code, Codex and opencode each needed the same eight decisions. When a
fourth arrives, work down this list — the differences between the three
existing answers are the useful part.

1. **Install shape.** Prefer a pinned tarball verified against a publisher
   checksum (jj, pnpm, opencode). Fall back to the vendor's install script only
   when the layout it produces is load-bearing (Claude's launcher symlink,
   Codex's standalone tree). Put the binary in `/usr/local/bin` — outside every
   volume mount and overlay target.
2. **Version pin.** Dockerfile `ARG` + `DEFAULT_*_VERSION` in xtask + a
   `--<tool>-version` flag. Never ride the installer's notion of "latest".
3. **Auto-update.** Every one of these ships an updater that will un-pin the
   build, usually failing partway because the binary is root-owned and the
   session is not. Find the environment variable; config-file switches have
   been unreliable in all three.
4. **State location.** Everything persistent goes in a subpath of the selected
   state namespace volume, and each new subpath must be added to
   `docker::ensure_subpaths` *and* the entrypoint's chown loop. That gives new
   tools namespace support without adding tool-specific CLI concepts. Check for
   a SQLite DB first (`ls` the data dir after one run): WAL demands the named
   volume and rules out host binds and overlay upperdirs.
5. **Credentials stay in the container.** No agent's credential store is
   bind-mounted from the host. Losing them to a dropped volume is an
   acceptable, reconstructible cost.
6. **Host config that isn't reconstructible** — instructions, agents, commands,
   skills — is bind-mounted read-only, per file or per subdirectory, never as a
   whole directory. Audit the list for credential-bearing files and leave those
   out. Check first whether the agent already reads something maudebox mounts:
   opencode needed no instruction mount at all.
7. **MCP.** Translate the shared `[mcp.NAME]` tables into the agent's dialect
   in `run.rs`, with a reserved `[mcp.NAME.<agent>]` sub-table for its own keys,
   stripped from every other agent's config. Establish where the file sits in
   that agent's precedence order before deciding what may go in it.
8. **Approval prompts.** The container is the sandbox; the agent shouldn't
   re-implement one. Put the setting in the lowest-precedence layer that works,
   so a project can still tighten it.

## Conventions for changes in this repo

### Ideas are evaluated and filed before implementation

An idea, complaint or "we should also" request is not automatically an
implementation order. Check it against `docs/SPEC.md`: it may already be
specified, contradict a deliberate boundary, or expose a genuine gap. File new
scope as an issue with current behavior, proposed behavior, constraints,
dependencies and acceptance criteria. Implementation is a separate decision.

This does not defer corrections to work in progress, defects relative to the
specification, or changes the user explicitly asked to implement.

### Keep the product specification consistent

`docs/SPEC.md` is the source of truth for shipped user-visible behavior and
deliberate product limits. Read the relevant section before changing behavior,
and update it in the same change whenever a feature is added, removed or
altered — it describes what ships in that change, not a future plan.

Preserve existing section anchors where possible; add subsections instead of
renumbering unrelated material. A future feature belongs in an issue until
adopted. If mentioning proposed behavior is necessary to explain a current
limit, label it clearly as unimplemented.

### Record decisions of consequence

`docs/decisions/` holds numbered decision records. Write one in the same change
when an approach wins over a viable alternative, establishes a hard-to-change
constraint, deliberately accepts a material cost, or deliberately declines a
capability. Choices with an obvious default and no live alternative don't need
one.

Accepted records are immutable. A reversal or material correction creates a new
record and marks the relationship at both ends and in the index. The
`Alternatives considered` section is required: record what lost and why.

### Technical conventions

- Keep the image multi-arch — any new tool install should branch on `uname -m`
  the way `mvnd` and `jj` do.
- The container runs as **root** (no `USER` directive), everything under
  `/root`. The entrypoint may drop privileges to the host UID at the end on
  Linux, but privileged setup still runs as root. Don't add a `USER` directive
  or replace the entrypoint's `setpriv` with a docker-CLI `--user` flag — that
  breaks the overlay mount, which needs real-root `CAP_SYS_ADMIN` in the user
  namespace.
- **Java's `user.home` ≠ `$HOME`.** Java derives `user.home` from
  `getpwuid()->pw_dir` in `/etc/passwd`, not the env var; Python's
  `pathlib.Path.home()` and Go's `os.UserHomeDir()` do the same. With root
  running and root's pw_dir being `/root`, this works out — but if you switch
  the runtime user or override `HOME`, expect Maven and friends to disagree
  about where `~` lives. Keep them aligned or pass `-Duser.home=…` explicitly.
- Don't replace the Claude binary handling with a plain `mv` of the installer's
  symlink, and keep the `~/.local/bin/claude → /usr/local/bin/claude` symlink.
- The overlayfs setup is the load-bearing trick when an `overlay` mount is
  requested. Don't replace it with a plain bind-mount or a `cp` of the source
  without understanding the isolation guarantees it provides.
- **Overlay copy-up preserves lowerdir UIDs.** On macOS/OrbStack, virtiofs is
  root-squashed so lowerdir appears as `uid=0` — which is why the container
  stays root there; dropping privileges would leave the upperdir unwritable. On
  Linux there's no squash, so copy-up yields host-uid-owned upperdir files the
  dropped-privilege process can write.
- Prefer semantic navigation and refactoring where available. Don't replace
  capability or behavior tests with brittle textual architecture tests.
- **Keep `README.md` in sync with user-visible changes in the same commit.**
  Whenever a change adds, removes or alters something the README documents —
  bundled tools or their versions, CLI subcommands/flags, the user-config schema
  (mounts, aliases), mount modes, overlay/state-volume behavior, the container
  user model, or the `cargo xtask` interface — update it alongside the code.
  `docs/SPEC.md` is the behavioral source of truth; the README is the public
  pitch and operating guide, and both must agree. Purely internal changes
  (refactors, private renames, test-only) need no update. When in doubt, grep
  both documents for the symbol, flag or section being touched.
