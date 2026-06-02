#!/bin/bash
# entrypoint.sh – optionally mounts a per-worktree overlayfs at a user-chosen
# path, sets up the persistent Claude state symlink, then exec's the user
# command. Runs as root for the parts that need it (overlay mount,
# /etc/passwd rewrite, chowns); on Linux drops to the host UID/GID before
# exec so writes to bind-mounted host paths don't end up root-owned on the
# host.
#
# Requires:
#   --cap-add SYS_ADMIN  (or --privileged)
#   --mount type=volume,src=maudebox-state,dst=/root/.claude,volume-subpath=claude
#   --mount type=volume,src=maudebox-state,dst=/root/.config/gh,volume-subpath=gh
#   --mount type=volume,src=maudebox-state,dst=/root/.ssh,volume-subpath=ssh
#       (all optional; together they persist Claude + gh login and SSH keys)
#   -v <project>:<project>          (bind-mount at the host path)
#   HOST_PROJECT_DIR=<project>      (env var; entrypoint adds a /root/<basename> symlink and cd's there)
#   HOST_UID, HOST_GID              (env vars; if set, drop privileges to those
#                                    before exec'ing the user command — Linux only)
#
# Optional (set together by the wrapper for each spec that uses overlay mode):
#   -v <host_path>:/maudebox/overlay-N/lower:ro
#   -v <per-worktree-volume>:/maudebox/overlay-N/upper
#   MAUDEBOX_OVERLAYS=<lower>:<upper>:<target>\n... (one triple per overlay)
#       The entrypoint loops over these and mounts each overlayfs. Without
#       this env var, no overlay setup happens at all.
#
set -euo pipefail

HOME_DIR="${HOME:-/root}"

# ── overlay setup (opt-in, multi) ─────────────────────────────────────────────
# The wrapper sets this up only for mount specs that requested `overlay` mode.
# MAUDEBOX_OVERLAYS holds one `lower:upper:target` triple per line. Without
# any overlays, /maudebox/overlay-* isn't even mounted and this block is a
# no-op.
if [ -n "${MAUDEBOX_OVERLAYS:-}" ]; then
    while IFS=':' read -r lower upper target; do
        [ -n "$lower" ] || continue
        if mountpoint -q "$target"; then
            echo "[entrypoint] $target already mounted, skipping"
            continue
        fi
        if [ ! -d "$lower" ] || [ -z "$(ls -A "$lower" 2>/dev/null)" ]; then
            echo "[entrypoint] $lower is empty/missing – skipping overlay -> $target"
            continue
        fi
        echo "[entrypoint] Setting up overlayfs: $lower -> $target"
        # upperdir and workdir must be on the same filesystem AND that fs must
        # not itself be overlayfs. The upper is a Docker named volume (host-fs
        # backed), so we put both subdirs inside it.
        mkdir -p "$target" "$upper/upper" "$upper/work"
        mount -t overlay overlay \
            -o "lowerdir=$lower,upperdir=$upper/upper,workdir=$upper/work" \
            "$target"
    done <<< "$MAUDEBOX_OVERLAYS"
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

# ── SSH dir permissions ───────────────────────────────────────────────────────
# ~/.ssh is a maudebox-state subpath shared across all containers (keys
# generated or added inside one container are reused by the next). Docker
# creates the subpath dir mode 0755; OpenSSH wants 0700 — and refuses keys
# under StrictModes if the dir is group/world-writable. Tighten it on every
# launch while still root: cheap and idempotent.
if [ -d "$HOME_DIR/.ssh" ]; then
    chmod 700 "$HOME_DIR/.ssh"
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

# ── terminal/pane title ───────────────────────────────────────────────────────
# Name the terminal after the maudebox instance (OSC 2) so a non-tmux terminal
# tab tracks the feature. prompt.sh re-asserts this on every interactive prompt;
# emitting it here also covers non-interactive launches (e.g. `maudebox . claude`)
# that never draw a bash prompt. Under tmux the window name (set host-side via
# `tmux rename-window`) is what the tab shows, not this.
if [ -n "${MAUDEBOX_INSTANCE:-}" ]; then
    printf '\033]2;%s\033\\' "$MAUDEBOX_INSTANCE"
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

    # `x` in /etc/passwd defers the password lookup to /etc/shadow. PAM's
    # account-management step (pam_unix.so account, pulled in by sudo via
    # common-account) reads shadow to validate the account isn't expired or
    # locked — and rejects the user with PAM_USER_UNKNOWN if there's no entry
    # at all. NOPASSWD in sudoers skips the *auth* step but not the *account*
    # step, so without a shadow entry every sudo invocation fails with
    # "account validation failure, is your account locked?". Write a locked-
    # password entry (`*` = no valid hash, can never log in via password) with
    # empty aging fields so the account never expires. Sudo's auth-bypass via
    # NOPASSWD then completes successfully.
    sed -i "/^host:/d" /etc/shadow
    echo 'host:*:::::::' >> /etc/shadow

    # Hand ownership of the writable trees to HOST_UID. Docker creates volumes
    # root-owned, and /root is root-owned from the image build, so without this
    # the dropped-privilege user can't write anything. -xdev keeps find from
    # crossing into bind-mounted host config (e.g. /root/.claude/CLAUDE.md,
    # which lives on a different fs and would refuse chown anyway). The
    # ! -uid test makes repeat runs no-ops. The /maudebox/overlay-*/upper
    # glob covers any per-overlay upper-layer volumes mounted by the wrapper;
    # nullglob keeps it harmless when no overlays are active.
    shopt -s nullglob
    for d in /maudebox/overlay-*/upper /root/.claude /root/.config/gh /root/.ssh /root; do
        [ -e "$d" ] || continue
        find "$d" -xdev \! -uid "$HOST_UID" \
            -exec chown -h "${HOST_UID}:${HOST_GID}" {} + 2>/dev/null || true
    done
    shopt -u nullglob

    exec setpriv --reuid="$HOST_UID" --regid="$HOST_GID" --clear-groups -- "$@"
fi

exec "$@"
