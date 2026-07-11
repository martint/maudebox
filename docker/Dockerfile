# syntax=docker/dockerfile:1
FROM eclipse-temurin:26-jdk-noble

ARG MVND_VERSION=1.0.6
ARG JJ_VERSION=0.43.0
ARG RUST_VERSION=1.96.1
ARG BUN_VERSION=1.3.14
ARG PNPM_VERSION=11.10.0
ARG CLAUDE_VERSION=2.1.205
ARG CODEX_VERSION=0.144.1
ARG PLAYWRIGHT_VERSION=1.61.1
ARG PLAYWRIGHT_MCP_VERSION=0.0.77

# The container starts as root for the privileged setup (overlay mount, volume
# chowns) and on Linux drops to the host UID before exec'ing the user command —
# Linux bind mounts pass UIDs through literally, so root-in-container writes
# would otherwise land as root on the host. macOS/OrbStack stays root for the
# whole session because virtiofs is root-squashed (host user appears as uid=0)
# and lowerdir-uid-preserving overlay copy-up would leave the ~/.m2 upperdir
# unwritable for any non-root user. See CLAUDE.md for the full story.
# Either way $HOME stays /root and that's where everything lives — claude
# config, the Maven cache mount, the project bind-mount.

# mvnd's daemon registry and logs default to ~/.m2/mvnd. Keep mvnd state
# out of the overlay so its small writes don't trigger pointless copy-up.
ENV MVND_DAEMON_STORAGE=/root/.mvnd

# `less` parses $LESS in addition to argv, so this makes any `less` invocation
# (including pagers hard-coded as `less -FX` in the user's mounted jj/git
# config) pass ANSI colors through instead of rendering them as literal
# `ESC[…m` text.
ENV LESS=-R

# Advertise 24-bit color support so tools that gate truecolor output on
# $COLORTERM (jj's diff renderer, bat, delta, etc.) emit full RGB escapes.
ENV COLORTERM=truecolor

# Claude Code's native binary lives in /usr/local/bin (already in PATH), but
# it also checks that ~/.local/bin is on PATH and warns if not — that's where
# its launcher symlink lives by convention. Put it on PATH to silence the
# warning; the symlink itself is created in the Claude Code install step.
ENV PATH=/root/.local/bin:$PATH

# Claude Code refuses --dangerously-skip-permissions when running as root
# unless IS_SANDBOX=1 signals the environment is already sandboxed. The
# container *is* the sandbox here (see the "Container runs as root" notes),
# so set it unconditionally.
ENV IS_SANDBOX=1

# Codex keeps all of its per-user state — auth.json, config.toml, sessions,
# and the SQLite state DB — under $CODEX_HOME (default ~/.codex). maudebox
# mounts a maudebox-state subpath there, so point CODEX_HOME at it explicitly.
# Codex hard-errors if CODEX_HOME is set to a path that doesn't exist, so the
# dir is created at build time too (covers a bare `docker run` with no volume).
ENV CODEX_HOME=/root/.codex

# Host gitconfig is bind-mounted read-only by the maudebox wrapper, so the user's identity
# and aliases work inside the container. But the host's commit.gpgsign=true
# (with 1Password's macOS-only ssh-sign program) would auto-fail every commit.
# These GIT_CONFIG_* env vars override the mounted config at runtime; the
# user can still bypass with `git -c commit.gpgsign=true commit ...` if they
# wire up agent forwarding themselves.
ENV GIT_CONFIG_COUNT=2 \
    GIT_CONFIG_KEY_0=commit.gpgsign  GIT_CONFIG_VALUE_0=false \
    GIT_CONFIG_KEY_1=tag.gpgSign     GIT_CONFIG_VALUE_1=false

# ── system packages ──────────────────────────────────────────────────────────
# gh ships from GitHub's own apt repo (cli.github.com/packages) which has
# both amd64 and arm64 debs, so no `uname -m` branching is needed. The repo
# is set up before the second apt-get update so gh installs in the same
# transaction as the base packages.
RUN apt-get update && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        gnupg \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && chmod a+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        gh \
        git \
        jq \
        less \
        linux-tools-generic \
        openssh-client \
        python-is-python3 \
        python3 \
        python3-dev \
        python3-pip \
        python3-venv \
        ripgrep \
        sudo \
        unzip \
        util-linux \
        vim \
    && rm -rf /var/lib/apt/lists/*

# Ubuntu's /usr/bin/perf is a wrapper that dispatches to
# /usr/lib/linux-tools/$(uname -r)/perf based on the *running* kernel. Inside a
# container uname -r reports the host kernel, which won't match the version the
# noble package shipped, so the wrapper bails with "perf not found for kernel".
# Symlink straight to the bundled binary to bypass the dispatch — perf is
# forward/backward compatible enough for the common cases (cycles, instructions,
# userspace stacks). Profiling beyond the host's perf_event_paranoid level needs
# `sudo perf …` since the Linux entrypoint drops CAP_PERFMON/CAP_SYS_ADMIN.
RUN ln -sf "$(ls -d /usr/lib/linux-tools/*/perf | head -1)" /usr/local/bin/perf

# On Linux the entrypoint drops to a "host" user at HOST_UID before exec'ing
# the user command, so privileged work (apt-get, mounts, etc.) needs sudo.
# Grant passwordless sudo to that fixed username; the rule is inert on
# macOS/OrbStack where the session stays root.
RUN echo 'host ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/host \
 && chmod 0440 /etc/sudoers.d/host

# Claude Code reads /etc/claude-code/managed-mcp.json as enterprise-managed
# MCP scope. The maudebox wrapper materializes this file from the user's
# config.toml [mcp.NAME] tables and bind-mounts it at the path below; pre-
# creating the parent dir lets the mount target's parent always exist even
# when no MCP servers are configured (the wrapper just skips the mount).
RUN mkdir -p /etc/claude-code

# Codex reads /etc/codex/config.toml as its lowest-precedence "system" config
# layer. The maudebox wrapper materializes it from the user's config.toml
# [mcp.NAME] tables (translated to Codex's [mcp_servers.NAME] shape) plus a
# sandbox default, and bind-mounts it there; the user's own ~/.codex/config.toml
# still overrides it. Pre-create /etc/codex so the mount target's parent exists,
# and /root/.codex so $CODEX_HOME resolves even without the state-volume mount.
RUN mkdir -p /etc/codex /root/.codex

# ── mvnd ──────────────────────────────────────────────────────────────────────
# Detect host arch so the image builds on both amd64 and arm64
RUN set -eux; \
    ARCH="$(uname -m)"; \
    case "$ARCH" in \
        x86_64)  MVND_ARCH="linux-amd64"  ;; \
        aarch64) MVND_ARCH="linux-aarch64" ;; \
        *)       echo "Unsupported arch: $ARCH" && exit 1 ;; \
    esac; \
    curl -fsSL \
        "https://downloads.apache.org/maven/mvnd/${MVND_VERSION}/maven-mvnd-${MVND_VERSION}-${MVND_ARCH}.zip" \
        -o /tmp/mvnd.zip \
    && unzip /tmp/mvnd.zip -d /opt \
    && mv "/opt/maven-mvnd-${MVND_VERSION}-${MVND_ARCH}" /opt/mvnd \
    && rm /tmp/mvnd.zip \
    && ln -s /opt/mvnd/bin/mvnd /usr/local/bin/mvnd \
    && ln -s /opt/mvnd/bin/mvnd /usr/local/bin/mvn

# ── Claude Code (native binary, no Node.js required) ─────────────────────────
# The installer drops a launcher symlink in ~/.local/bin/claude pointing into
# ~/.local/share/claude/versions/<ver>. We resolve the symlink and move the
# real binary to /usr/local/bin so it lives outside any volume mount path,
# then re-point the launcher symlink at it: the native binary records its
# installMethod and warns at runtime if ~/.local/bin/claude is missing.
RUN curl -fsSL https://claude.ai/install.sh | bash -s -- "$CLAUDE_VERSION" \
 && CLAUDE_LINK="$(ls "$HOME/.local/bin/claude" "$HOME/.claude/bin/claude" 2>/dev/null | head -1)" \
 && CLAUDE_BIN="$(readlink -f "$CLAUDE_LINK")" \
 && mv "$CLAUDE_BIN" /usr/local/bin/claude \
 && chmod a+rx /usr/local/bin/claude \
 && rm -rf "$HOME/.local/share/claude" "$HOME/.claude" \
 && ln -sf /usr/local/bin/claude "$HOME/.local/bin/claude"

# ── Codex (installer-managed "standalone" layout) ────────────────────────────
# `codex remote-control start` — the daemon that lets the ChatGPT app (and a
# local `codex --remote` TUI) drive a session — refuses to run against a plain
# binary. It requires the standalone install the official installer lays down at
# $CODEX_HOME/packages/standalone/current/codex, because it starts (and updates)
# the app-server from that fixed path. Reproduce that layout from the release
# "package" bundle, which also carries the vendored bwrap and rg codex expects
# (so the daemon's sandbox helper is present without an apt install).
#
# Stage it under /opt, outside the maudebox-state volume: $CODEX_HOME is a volume
# subpath that starts empty at runtime, so a build-time install there would be
# shadowed by the mount. The entrypoint symlinks the volume-backed
# $CODEX_HOME/packages/standalone/current at this /opt release on first launch —
# a symlink, not a ~336 MB copy into the volume. The interactive `codex` on PATH
# is the same standalone binary; `current` is version-agnostic so an image
# rebuild with a new CODEX_VERSION is picked up without touching the volume.
RUN set -eux; \
    ARCH="$(uname -m)"; \
    case "$ARCH" in \
        x86_64)  CODEX_TARGET="x86_64-unknown-linux-musl"  ;; \
        aarch64) CODEX_TARGET="aarch64-unknown-linux-musl" ;; \
        *)       echo "Unsupported arch: $ARCH" && exit 1 ;; \
    esac; \
    REL="/opt/codex/packages/standalone/releases/${CODEX_VERSION}-${CODEX_TARGET}"; \
    mkdir -p "$REL"; \
    curl -fsSL \
        "https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/codex-package-${CODEX_TARGET}.tar.gz" \
        -o /tmp/codex-package.tar.gz \
    && tar -xzf /tmp/codex-package.tar.gz -C "$REL" \
    && rm /tmp/codex-package.tar.gz \
    && ln -sf bin/codex "$REL/codex" \
    && ln -sf "releases/${CODEX_VERSION}-${CODEX_TARGET}" /opt/codex/packages/standalone/current \
    && chmod -R a+rX /opt/codex \
    && ln -sf /opt/codex/packages/standalone/current/codex /usr/local/bin/codex

# ── jujutsu ───────────────────────────────────────────────────────────────────
# Use musl binaries: statically linked, no libc version dependency
RUN set -eux; \
    ARCH="$(uname -m)"; \
    case "$ARCH" in \
        x86_64)  JJ_ARCH="x86_64-unknown-linux-musl"  ;; \
        aarch64) JJ_ARCH="aarch64-unknown-linux-musl"  ;; \
        *)       echo "Unsupported arch: $ARCH" && exit 1 ;; \
    esac; \
    curl -fsSL \
        "https://github.com/jj-vcs/jj/releases/download/v${JJ_VERSION}/jj-v${JJ_VERSION}-${JJ_ARCH}.tar.gz" \
        -o /tmp/jj.tar.gz \
    && tar -xzf /tmp/jj.tar.gz -C /usr/local/bin ./jj \
    && chmod +x /usr/local/bin/jj \
    && rm /tmp/jj.tar.gz

# ── rust ─────────────────────────────────────────────────────────────────────
# Install via rustup into /usr/local so the toolchain lives outside any volume
# mount — an opt-in `~/.cargo` overlay would otherwise hide the binaries.
# CARGO_HOME and RUSTUP_HOME are set permanently (matches the official
# rust:slim image); a+w on both lets the Linux dropped-privilege user populate
# the registry cache and run `cargo install` without sudo. The rustup-init
# binary is the smallest bootstrap surface — fetch the pinned version, run
# once with a fixed default toolchain, then drop it.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH

RUN set -eux; \
    ARCH="$(uname -m)"; \
    case "$ARCH" in \
        x86_64)  RUST_ARCH="x86_64-unknown-linux-gnu"  ;; \
        aarch64) RUST_ARCH="aarch64-unknown-linux-gnu" ;; \
        *)       echo "Unsupported arch: $ARCH" && exit 1 ;; \
    esac; \
    RUSTUP_VERSION="1.27.1"; \
    curl --proto '=https' --tlsv1.2 -fsSL \
        "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${RUST_ARCH}/rustup-init" \
        -o /tmp/rustup-init \
    && chmod +x /tmp/rustup-init \
    && /tmp/rustup-init -y \
        --no-modify-path \
        --profile minimal \
        --default-toolchain "${RUST_VERSION}" \
        --default-host "${RUST_ARCH}" \
    && rm /tmp/rustup-init \
    && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# ── bun ──────────────────────────────────────────────────────────────────────
# Single statically-linked binary inside a zip; arch-suffixed asset names
# follow the same `uname -m` mapping as mvnd/jj. The glibc build matches the
# noble base; bun also ships a `-musl` variant we deliberately don't use.
RUN set -eux; \
    ARCH="$(uname -m)"; \
    case "$ARCH" in \
        x86_64)  BUN_ARCH="x64"     ;; \
        aarch64) BUN_ARCH="aarch64" ;; \
        *)       echo "Unsupported arch: $ARCH" && exit 1 ;; \
    esac; \
    curl -fsSL \
        "https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/bun-linux-${BUN_ARCH}.zip" \
        -o /tmp/bun.zip \
    && unzip -j /tmp/bun.zip "bun-linux-${BUN_ARCH}/bun" -d /usr/local/bin \
    && chmod +x /usr/local/bin/bun \
    && rm /tmp/bun.zip

# ── pnpm ─────────────────────────────────────────────────────────────────────
# pnpm 11 ships a small native launcher plus a `dist/` tree of JS the launcher
# exec's into — they must stay co-located, so we extract the whole tarball to
# /opt/pnpm and symlink the launcher onto PATH (same shape as mvnd).
RUN set -eux; \
    ARCH="$(uname -m)"; \
    case "$ARCH" in \
        x86_64)  PNPM_ARCH="x64"   ;; \
        aarch64) PNPM_ARCH="arm64" ;; \
        *)       echo "Unsupported arch: $ARCH" && exit 1 ;; \
    esac; \
    mkdir -p /opt/pnpm \
    && curl -fsSL \
        "https://github.com/pnpm/pnpm/releases/download/v${PNPM_VERSION}/pnpm-linux-${PNPM_ARCH}.tar.gz" \
        -o /tmp/pnpm.tar.gz \
    && tar -xzf /tmp/pnpm.tar.gz -C /opt/pnpm \
    && rm /tmp/pnpm.tar.gz \
    && ln -s /opt/pnpm/pnpm /usr/local/bin/pnpm

# Make `bun install -g …` and `pnpm install -g …` discoverable without further
# setup. bun's global-install dir defaults to ~/.bun/bin; pnpm requires
# PNPM_HOME to be set and on PATH or it prints "Run `pnpm setup`…" and globals
# land in a dir the shell can't find. Both dirs also make convenient overlay
# targets — add `~/.bun:~/.bun:overlay` or
# `~/.local/share/pnpm:~/.local/share/pnpm:overlay` to your config to give
# each worktree its own writable cache layered over a shared host warm-start.
# Pin bun's install root so it doesn't depend on $HOME being set (a Docker
# build can run with HOME unset, and bun then aborts with "No global directory
# found"). /root/.bun is the same dir bun's default ($HOME/.bun) resolves to at
# runtime, so build and runtime agree.
ENV BUN_INSTALL=/root/.bun
ENV PNPM_HOME=/root/.local/share/pnpm
ENV PATH=/root/.bun/bin:/opt/bun/bin:/root/.local/share/pnpm:$PATH

# ── Playwright (browser automation + MCP for Claude) ──────────────────────────
# No Node.js lives in this image, so Playwright runs under bun. `bun install -g`
# fetches the `playwright` CLI and the `@playwright/mcp` server but only links
# their package `cli.js` into <root>/bin via a `#!/usr/bin/env node` shebang
# symlink — which can't run here. So we replace those two bin entries with
# wrappers that `exec bun <cli.js>`; bun ignores the shebang and executes the JS
# directly. We resolve each symlink to its real cli.js and `rm` the symlink
# *before* writing the wrapper — writing through the still-present symlink would
# clobber cli.js itself (the wrapper would then point at its own text). The
# server's bin is `playwright-mcp` — point an `[mcp.playwright]` table at it in
# your config.toml to let Claude Code drive a browser.
#
# This install root is /opt/bun, NOT the runtime BUN_INSTALL (/root/.bun), and
# the distinction is load-bearing: the Linux entrypoint chowns the whole /root
# tree to the host UID on every launch, so a couple-thousand-file Playwright
# install under /root/.bun would be re-chowned on each fresh container — a
# visible pre-prompt pause. Keeping it under /opt (world-readable/executable via
# `chmod -R a+rX`, like the Rust toolchain under /usr/local and the browsers
# under /ms-playwright) sits it outside that sweep. /root/.bun stays empty at
# build time for the user's own runtime `bun install -g`. Both bin dirs are on
# PATH, with /root/.bun/bin first so user globals win.
#
# Browsers install into a system path (PLAYWRIGHT_BROWSERS_PATH) rather than the
# per-user default under ~/.cache, so they sit outside any volume or overlay and
# every worktree shares one read-only Chromium instead of re-downloading per
# container. `--with-deps` apt-installs Chromium's shared libraries (we're still
# root at build time). Only Chromium is bundled; run `playwright install firefox
# webkit` by hand if you need the others. Chromium ships native amd64 and arm64
# builds, so no `uname -m` branching is needed — Playwright fetches the build
# matching the host arch.
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
RUN set -eux; \
    export BUN_INSTALL=/opt/bun; \
    bun install -g \
        "playwright@${PLAYWRIGHT_VERSION}" \
        "@playwright/mcp@${PLAYWRIGHT_MCP_VERSION}"; \
    for name in playwright playwright-mcp; do \
        cli="$(readlink -f "$BUN_INSTALL/bin/$name")"; \
        rm "$BUN_INSTALL/bin/$name"; \
        printf '#!/bin/sh\nexec bun %s "$@"\n' "$cli" > "$BUN_INSTALL/bin/$name"; \
        chmod +x "$BUN_INSTALL/bin/$name"; \
    done; \
    chmod -R a+rX "$BUN_INSTALL"; \
    playwright install --with-deps chromium; \
    chmod -R a+rX "$PLAYWRIGHT_BROWSERS_PATH"; \
    rm -rf /var/lib/apt/lists/*

# Overlay mount points (e.g. /maudebox/overlay-N/{lower,upper}) are created
# at runtime by Docker when the wrapper passes -v for them, so no build-time
# mkdir is needed. The merged overlayfs target is whatever path the user
# picks in the mount spec; the entrypoint creates it on demand. The project
# itself is bind-mounted by the maudebox wrapper at its *host* path (so
# jj/git worktree metadata resolves), and the entrypoint adds a
# /root/<basename> symlink pointing at it.

# ── shell init: prompt + user aliases ────────────────────────────────────────
# /etc/bash.bashrc is sourced first by interactive non-login shells; /root/.bashrc
# from the noble base then sets its own PS1, which would override ours. Source
# our scripts from both — /root/.bashrc runs last and wins.
#
# aliases.sh consumes the MAUDEBOX_ALIASES env var the wrapper builds from
# the user's ~/.maudebox `[aliases]` table, installing each entry as a bash
# alias.
COPY --chmod=0644 prompt.sh  /etc/profile.d/dev-prompt.sh
COPY --chmod=0644 aliases.sh /etc/profile.d/maudebox-aliases.sh
RUN { echo '. /etc/profile.d/dev-prompt.sh';        echo '. /etc/profile.d/maudebox-aliases.sh'; } >> /etc/bash.bashrc \
 && { echo '. /etc/profile.d/dev-prompt.sh';        echo '. /etc/profile.d/maudebox-aliases.sh'; } >> /root/.bashrc

# ── in-container helper for `maudebox keep` ───────────────────────────────────
# Lets the user preserve an ephemeral workspace from inside the container by
# touching a flag file in the per-instance state dir bind-mounted by the host
# wrapper. Inert outside ephemeral runs (the dir isn't mounted).
COPY --chmod=0755 maudebox-keep /usr/local/bin/maudebox-keep

# ── entrypoint ────────────────────────────────────────────────────────────────
COPY --chmod=0755 entrypoint.sh /usr/local/bin/entrypoint.sh

WORKDIR /root

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["bash"]
