# syntax=docker/dockerfile:1
FROM eclipse-temurin:25-jdk-noble

ARG MVND_VERSION=1.0.5
ARG JJ_VERSION=0.34.0

# The container runs as root throughout. OrbStack root-squashes virtiofs so
# every file in the host bind-mounts (project source, ~/.m2) appears as
# uid=0 inside the container; running anything but root means overlayfs
# copy-up creates root-owned files that the dropped-privilege user can't
# write to. With root, $HOME is /root by default and that's where everything
# lives — claude config, the Maven cache mount, the project bind-mount.

# mvnd's daemon registry and logs default to ~/.m2/mvnd. Keep mvnd state
# out of the overlay so its small writes don't trigger pointless copy-up.
ENV MVND_DAEMON_STORAGE=/root/.mvnd

# `less` parses $LESS in addition to argv, so this makes any `less` invocation
# (including pagers hard-coded as `less -FX` in the user's mounted jj/git
# config) pass ANSI colors through instead of rendering them as literal
# `ESC[…m` text.
ENV LESS=-R

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
RUN apt-get update && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        git \
        less \
        ripgrep \
        unzip \
        util-linux \
        vim \
    && rm -rf /var/lib/apt/lists/*

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
# real binary to /usr/local/bin so it lives outside any volume mount path.
RUN curl -fsSL https://claude.ai/install.sh | bash \
 && CLAUDE_LINK="$(ls "$HOME/.local/bin/claude" "$HOME/.claude/bin/claude" 2>/dev/null | head -1)" \
 && CLAUDE_BIN="$(readlink -f "$CLAUDE_LINK")" \
 && mv "$CLAUDE_BIN" /usr/local/bin/claude \
 && chmod a+rx /usr/local/bin/claude \
 && rm -rf "$HOME/.local/share/claude" "$HOME/.local/bin/claude" "$HOME/.claude"

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

# ── overlay directories (created root-owned, mounted at runtime) ──────────────
# /m2-host    – bind-mount of host ~/.m2 (ro lower layer)
# /m2-upper   – named-volume root; must be on a non-overlay fs because
#               overlayfs rejects another overlay as upperdir/workdir. The
#               entrypoint creates ./upper and ./work inside it.
# /root/.m2   – the merged mount point
# The project itself is bind-mounted by the maudebox wrapper at its *host* path (so jj/git
# worktree metadata resolves), and the entrypoint adds a /root/<basename>
# symlink pointing at it.
RUN mkdir -p /m2-host /m2-upper /root/.m2

# ── shell prompt with jj/git VCS info ─────────────────────────────────────────
# /etc/bash.bashrc is sourced first by interactive non-login shells; /root/.bashrc
# from the noble base then sets its own PS1, which would override ours. Source
# our prompt.sh from both — /root/.bashrc runs last and wins.
COPY --chmod=0644 prompt.sh /etc/profile.d/dev-prompt.sh
RUN echo '. /etc/profile.d/dev-prompt.sh' >> /etc/bash.bashrc \
 && echo '. /etc/profile.d/dev-prompt.sh' >> /root/.bashrc

# ── entrypoint ────────────────────────────────────────────────────────────────
COPY --chmod=0755 entrypoint.sh /usr/local/bin/entrypoint.sh

WORKDIR /root

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["bash"]
