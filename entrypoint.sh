#!/bin/bash
# entrypoint.sh – mounts overlayfs over ~/.m2, sets up the persistent Claude
# state symlink, then exec's the user command. Runs as root throughout: see
# the Dockerfile comment above MVND_DAEMON_STORAGE for the rationale.
#
# Requires:
#   --cap-add SYS_ADMIN  (or --privileged)
#   -v ~/.m2:/m2-host:ro
#   -v <per-worktree>:/m2-upper
#   -v maudebox-claude:/root/.claude   (optional, persists Claude login)
#   -v <project>:<project>          (bind-mount at the host path)
#   HOST_PROJECT_DIR=<project>      (env var; entrypoint adds a /root/<basename> symlink and cd's there)
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

exec "$@"
