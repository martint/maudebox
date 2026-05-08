#!/bin/bash
# entrypoint.sh – mounts overlayfs over ~/.m2, sets up the persistent Claude
# state symlink, then exec's the user command. Runs as root for the parts that
# need it (overlay mount, /etc/passwd rewrite, chowns); on Linux drops to the
# host UID/GID before exec so writes to bind-mounted host paths don't end up
# root-owned on the host.
#
# Requires:
#   --cap-add SYS_ADMIN  (or --privileged)
#   -v ~/.m2:/m2-host:ro
#   -v <per-worktree>:/m2-upper
#   --mount type=volume,src=maudebox-state,dst=/root/.claude,volume-subpath=claude
#   --mount type=volume,src=maudebox-state,dst=/root/.config/gh,volume-subpath=gh
#       (both optional; together they persist Claude + gh login)
#   -v <project>:<project>          (bind-mount at the host path)
#   HOST_PROJECT_DIR=<project>      (env var; entrypoint adds a /root/<basename> symlink and cd's there)
#   HOST_UID, HOST_GID              (env vars; if set, drop privileges to those
#                                    before exec'ing the user command — Linux only)
#
set -euo pipefail

HOME_DIR="${HOME:-/root}"
M2_MOUNT="$HOME_DIR/.m2"

# ── overlay setup ─────────────────────────────────────────────────────────────
if mountpoint -q "$M2_MOUNT"; then
    echo "[entrypoint] $M2_MOUNT already mounted, skipping overlayfs setup"
elif [ -d /m2-host ] && [ "$(ls -A /m2-host 2>/dev/null)" ]; then
    echo "[entrypoint] Setting up overlayfs: lowerdir=/m2-host -> $M2_MOUNT"
    # upperdir and workdir must be on the same filesystem AND that fs must not
    # itself be overlayfs. /m2-upper is a Docker named volume (host-fs backed),
    # so we put both subdirs inside it.
    mkdir -p /m2-upper/upper /m2-upper/work
    mount -t overlay overlay \
        -o "lowerdir=/m2-host,upperdir=/m2-upper/upper,workdir=/m2-upper/work" \
        "$M2_MOUNT"
    echo "[entrypoint] overlayfs mounted"
else
    echo "[entrypoint] /m2-host is empty or absent – skipping overlay"
fi

# ── Claude state symlink ──────────────────────────────────────────────────────
# Redirect ~/.claude.json (login token + project state) into the persistent
# ~/.claude volume so authentication survives container restarts.
CLAUDE_DIR="$HOME_DIR/.claude"
if [ -d "$CLAUDE_DIR" ]; then
    CLAUDE_STATE="$CLAUDE_DIR/state.json"
    if [ ! -s "$CLAUDE_STATE" ]; then
        (umask 077 && echo '{}' > "$CLAUDE_STATE")
    fi
    if [ ! -L "$HOME_DIR/.claude.json" ] || [ "$(readlink -- "$HOME_DIR/.claude.json")" != "$CLAUDE_STATE" ]; then
        rm -f "$HOME_DIR/.claude.json"
        ln -s "$CLAUDE_STATE" "$HOME_DIR/.claude.json"
    fi
fi

# ── /root/<basename> convenience symlink + initial cwd ───────────────────────
# The project (and any jj/git base repo) is bind-mounted at its host path so
# absolute paths in worktree/workspace metadata resolve. Also expose it as
# /root/<basename> for short cd's, and start the user's shell *inside* that
# symlink so $PWD shows the friendly path while the underlying filesystem is
# still the bind-mounted host path.
if [ -n "${HOST_PROJECT_DIR:-}" ]; then
    PROJECT_LINK="$HOME_DIR/$(basename "$HOST_PROJECT_DIR")"
    if [ "$PROJECT_LINK" != "$HOST_PROJECT_DIR" ] && [ ! -e "$PROJECT_LINK" ]; then
        ln -s "$HOST_PROJECT_DIR" "$PROJECT_LINK"
    fi
    if [ -d "$PROJECT_LINK" ]; then
        cd "$PROJECT_LINK"
    fi
fi

# ── drop privileges (Linux) ───────────────────────────────────────────────────
# On Linux Docker, container UID 0 maps to host UID 0 through bind mounts, so
# any file the container writes into a bind-mounted host path lands as root on
# the host. The wrapper passes HOST_UID/HOST_GID on Linux so we can drop to
# those before exec'ing the user command. macOS/OrbStack root-squashes virtiofs
# and translates container-root↔host-user automatically, so the wrapper leaves
# these unset and we stay root.
if [ -n "${HOST_UID:-}" ] && [ -n "${HOST_GID:-}" ] && [ "$HOST_UID" != "0" ]; then
    # Java's user.home (and Python's Path.home(), Go's os.UserHomeDir(), etc.)
    # reads pw_dir from /etc/passwd by UID — not $HOME. Ensure the entry for
    # HOST_UID points at /root so Maven and friends agree with the rest of the
    # tooling about where ~ is. The noble base ships a `ubuntu` user at UID
    # 1000 with home /home/ubuntu, which is the most common collision.
    sed -i "/^[^:]*:[^:]*:${HOST_UID}:/d" /etc/passwd
    echo "host:x:${HOST_UID}:${HOST_GID}::/root:/bin/bash" >> /etc/passwd
    sed -i "/^[^:]*:[^:]*:${HOST_GID}:/d" /etc/group
    echo "host:x:${HOST_GID}:" >> /etc/group

    # Hand ownership of the writable trees to HOST_UID. Docker creates volumes
    # root-owned, and /root is root-owned from the image build, so without this
    # the dropped-privilege user can't write anything. -xdev keeps find from
    # crossing into bind-mounted host config (e.g. /root/.claude/CLAUDE.md,
    # which lives on a different fs and would refuse chown anyway). The
    # ! -uid test makes repeat runs no-ops.
    for d in /m2-upper /root/.claude /root/.config/gh /root; do
        [ -e "$d" ] || continue
        find "$d" -xdev \! -uid "$HOST_UID" \
            -exec chown -h "${HOST_UID}:${HOST_GID}" {} + 2>/dev/null || true
    done

    exec setpriv --reuid="$HOST_UID" --regid="$HOST_GID" --clear-groups -- "$@"
fi

exec "$@"
