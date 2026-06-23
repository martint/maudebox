use crate::config::{
    read_aliases, read_default_command, read_mcp_servers, read_mounts, read_network,
};
use crate::docker;
use crate::mount::{build_mount_plan, MountPlan};
use crate::paths::{canonicalize, home, xdg_state_home, STATE_VOLUME};
use crate::vcs::detect_vcs_base;
use crate::volume::{compute_overlay_volume, compute_state_dir};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub struct RunOptions {
    pub image: String,
    pub memory_from: String,
    pub extra_mounts: Vec<String>,
    /// Discriminator appended to the project basename to form
    /// `MAUDEBOX_INSTANCE` and the `maudebox.instance` label. Empty = no
    /// suffix (just basename). See `--instance` on the CLI.
    pub instance: String,
    pub ephemeral_name: String,
    /// Docker networks to join at launch (`docker run --network`, one flag
    /// each). Empty = fall back to the config-file `network` key, else
    /// Docker's default bridge. See `--network` on the CLI.
    pub network: Vec<String>,
    pub project_dir: String,
    pub command: Vec<String>,
}

pub fn run(opts: RunOptions) -> Result<i32> {
    let project_dir = canonicalize(&opts.project_dir);

    if !project_dir.is_dir() {
        eprintln!("Not a directory: {}", project_dir.display());
        return Ok(1);
    }

    // Fall back to the configured default command when the user gave none,
    // so a plain `maudebox` / `maudebox new` launches it instead of a shell.
    let command = if opts.command.is_empty() {
        read_default_command()?
    } else {
        opts.command
    };
    let command = resolve_alias(command)?;
    let vcs_base = detect_vcs_base(&project_dir);

    // ── persistent state (maudebox-state volume + read-only host overlays) ──
    docker::ensure_volume(STATE_VOLUME, &[])?;
    docker::ensure_subpaths(&opts.image, STATE_VOLUME, &["claude", "gh", "ssh"])?;

    let mut claude_mounts: Vec<String> = vec![
        "--mount".into(),
        format!("type=volume,src={STATE_VOLUME},dst=/root/.claude,volume-subpath=claude"),
        "--mount".into(),
        format!("type=volume,src={STATE_VOLUME},dst=/root/.config/gh,volume-subpath=gh"),
        // A container-only ~/.ssh shared across all maudebox instances — keys
        // generated/added inside one container are reused by the next. The
        // host's ~/.ssh is deliberately never mounted; this is separate.
        "--mount".into(),
        format!("type=volume,src={STATE_VOLUME},dst=/root/.ssh,volume-subpath=ssh"),
    ];
    let host_claude_dir = home()?.join(".claude");
    for p in ["CLAUDE.md", "settings.json", "agents", "commands", "skills", "plugins"] {
        let host_path = host_claude_dir.join(p);
        if host_path.exists() {
            claude_mounts.push("-v".into());
            claude_mounts.push(format!(
                "{}:/root/.claude/{p}:ro",
                host_path.display()
            ));
        }
    }

    // Auto-memory: bind-mount the host's projects/<key>/memory rw so memories
    // round-trip between host and container. --memory-from breaks the host/
    // container symmetry: container target is keyed by project_dir (Claude
    // Code derives it from cwd inside the container) but the host source can
    // be keyed elsewhere, so an ephemeral workspace can share its parent's
    // memory.
    let project_key = encode_project_key(&project_dir.to_string_lossy());
    let memory_source: PathBuf = if opts.memory_from.is_empty() {
        project_dir.clone()
    } else {
        PathBuf::from(&opts.memory_from)
    };
    let memory_source_key = encode_project_key(&memory_source.to_string_lossy());
    let host_memory_dir = host_claude_dir
        .join("projects")
        .join(&memory_source_key)
        .join("memory");
    fs::create_dir_all(&host_memory_dir)?;
    claude_mounts.push("-v".into());
    claude_mounts.push(format!(
        "{}:/root/.claude/projects/{project_key}/memory",
        host_memory_dir.display()
    ));

    // ── host VCS config (read-only) ────────────────────────────────────────
    let mut vcs_config_mounts: Vec<String> = Vec::new();
    let h = home()?;
    let gitconfig = h.join(".gitconfig");
    let git_config_dir = h.join(".config").join("git");
    let jj_config = h.join(".config").join("jj").join("config.toml");
    if gitconfig.exists() {
        vcs_config_mounts.push("-v".into());
        vcs_config_mounts.push(format!("{}:/root/.gitconfig:ro", gitconfig.display()));
    }
    if git_config_dir.exists() {
        vcs_config_mounts.push("-v".into());
        vcs_config_mounts
            .push(format!("{}:/root/.config/git:ro", git_config_dir.display()));
    }
    if jj_config.exists() {
        vcs_config_mounts.push("-v".into());
        vcs_config_mounts.push(format!(
            "{}:/root/.config/jj/config.toml:ro",
            jj_config.display()
        ));
    }

    // ── managed MCP servers (config-file [mcp.NAME] tables) ────────────────
    // Translate the user's TOML into a managed-mcp.json on the host and
    // bind-mount it at /etc/claude-code/managed-mcp.json — Claude Code reads
    // that path as enterprise-managed scope, so servers apply to every
    // session without per-launch CLI work. No-op when the config has no
    // [mcp.*] tables.
    let mut managed_mcp_mount: Vec<String> = Vec::new();
    if let Some(servers) = read_mcp_servers()? {
        let host_path = write_managed_mcp_json(&servers)?;
        managed_mcp_mount.push("-v".into());
        managed_mcp_mount.push(format!(
            "{}:/etc/claude-code/managed-mcp.json:ro",
            host_path.display()
        ));
    }

    // ── extra mounts (CLI --mount + config-file mounts) ────────────────────
    let mut all_specs = opts.extra_mounts.clone();
    all_specs.extend(read_mounts()?);
    let MountPlan {
        mount_args: extra_mount_args,
        overlays,
    } = build_mount_plan(&all_specs)?;

    // ── aliases env var (parsed by /etc/profile.d/maudebox-aliases.sh) ─────
    let mut aliases_env: Vec<String> = Vec::new();
    let aliases = read_aliases()?;
    if !aliases.is_empty() {
        let mut val = String::new();
        for (n, v) in &aliases {
            val.push_str(&format!("{n}={v}\n"));
        }
        aliases_env.push("-e".into());
        aliases_env.push(format!("MAUDEBOX_ALIASES={val}"));
    }

    // ── labels (instance/project/ephemeral) ────────────────────────────────
    // Default instance handle is the project basename. `--instance review` on
    // a `trino` project gives `trino-review` — two concurrent containers on
    // the same project then have distinct $MAUDEBOX_INSTANCE values so an
    // alias like `claude --remote-control $MAUDEBOX_INSTANCE` doesn't collide.
    let basename = project_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let instance_name = instance_handle(&basename, &opts.instance);
    let mut label_strs: Vec<String> = vec![
        format!("maudebox.instance={instance_name}"),
        format!("maudebox.project={}", project_dir.display()),
    ];
    let mut ephemeral_mount: Vec<String> = Vec::new();
    if !opts.ephemeral_name.is_empty() {
        let state_dir = compute_state_dir(&project_dir)?;
        fs::create_dir_all(&state_dir)?;
        label_strs.push("maudebox.ephemeral=true".into());
        label_strs.push(format!("maudebox.ephemeral-name={}", opts.ephemeral_name));
        label_strs.push(format!("maudebox.state-dir={}", state_dir.display()));
        ephemeral_mount.push("-v".into());
        ephemeral_mount.push(format!("{}:/run/maudebox", state_dir.display()));
    } else {
        label_strs.push("maudebox.ephemeral=false".into());
    }

    // The `--label` flags for `docker run` are interleaved label_strs.
    let label_args: Vec<String> = label_strs
        .iter()
        .flat_map(|l| ["--label".to_string(), l.clone()])
        .collect();

    // ── overlay volumes + entrypoint env ───────────────────────────────────
    let mut overlay_args: Vec<String> = Vec::new();
    let mut overlay_env: Vec<String> = Vec::new();
    if !overlays.is_empty() {
        let mut env_val = String::new();
        for (i, o) in overlays.iter().enumerate() {
            let n = i + 1;
            let vol = compute_overlay_volume(&project_dir, &o.container_dst);
            let mut volume_labels: Vec<String> = label_strs.clone();
            volume_labels.push(format!("maudebox.overlay-target={}", o.container_dst));
            docker::ensure_volume(&vol, &volume_labels)?;
            overlay_args.push("-v".into());
            overlay_args.push(format!("{}:/maudebox/overlay-{n}/lower:ro", o.host_src));
            overlay_args.push("-v".into());
            overlay_args.push(format!("{vol}:/maudebox/overlay-{n}/upper"));
            env_val.push_str(&format!(
                "/maudebox/overlay-{n}/lower:/maudebox/overlay-{n}/upper:{}\n",
                o.container_dst
            ));
        }
        overlay_env.push("-e".into());
        overlay_env.push(format!("MAUDEBOX_OVERLAYS={env_val}"));
    }

    // ── project + base-repo mounts (host paths preserved) ──────────────────
    let mut project_mounts: Vec<String> = vec![
        "-v".into(),
        format!("{}:{}", project_dir.display(), project_dir.display()),
    ];
    if let Some(base) = &vcs_base {
        if base != &project_dir {
            project_mounts.push("-v".into());
            project_mounts.push(format!("{}:{}", base.display(), base.display()));
        }
    }

    // ── env: UID/GID on Linux, terminal forwarding ─────────────────────────
    let mut user_env: Vec<String> = Vec::new();
    if cfg!(target_os = "linux") {
        user_env.push("-e".into());
        user_env.push(format!("HOST_UID={}", get_uid()));
        user_env.push("-e".into());
        user_env.push(format!("HOST_GID={}", get_gid()));
    }
    let mut term_env: Vec<String> = vec!["-e".into(), "TERM_PROGRAM=tmux".into()];
    for v in ["COLORTERM", "LC_TERMINAL", "LC_TERMINAL_VERSION"] {
        if let Ok(val) = std::env::var(v) {
            if !val.is_empty() {
                term_env.push("-e".into());
                term_env.push(format!("{v}={val}"));
            }
        }
    }

    // ── status preamble ────────────────────────────────────────────────────
    println!("Image    : {}", opts.image);
    println!("Worktree : {}", project_dir.display());
    if let Some(base) = &vcs_base {
        if base != &project_dir {
            println!("VCS base : {}", base.display());
        }
    }
    for o in &overlays {
        println!("Overlay  : {} -> {}", o.host_src, o.container_dst);
    }

    // ── network ────────────────────────────────────────────────────────────
    // Join one or more user-defined Docker networks (e.g. a compose project's
    // network) so containers can reach a service addressed by container name
    // without publishing any port to the host. CLI `--network` wins over the
    // config `network` key; unset leaves Docker's default bridge. Repeating
    // `--network` on `docker run` requires Docker 23+ (maudebox already needs
    // 25+). `host-gateway` (set below via --add-host) still resolves on
    // user-defined networks.
    let networks = if opts.network.is_empty() {
        read_network()?
    } else {
        opts.network.clone()
    };
    let network_args: Vec<String> = networks
        .iter()
        .flat_map(|n| ["--network".to_string(), n.clone()])
        .collect();
    if !networks.is_empty() {
        println!("Network  : {}", networks.join(", "));
    }

    // ── exec docker run ────────────────────────────────────────────────────
    // `--name` makes `docker ps`/`exec`/`logs`/`stop` refer to the
    // container by the same handle as `$MAUDEBOX_INSTANCE` and the
    // `maudebox.instance` label, instead of a random `clever_curie`. A
    // collision (two concurrent containers picking the same name) surfaces
    // as a docker error; pass `--instance NAME` to discriminate, exactly
    // as for `$MAUDEBOX_INSTANCE` collisions.
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-it".into(),
        "--name".into(),
        instance_name.clone(),
        "--cap-add".into(),
        "SYS_ADMIN".into(),
        "--security-opt".into(),
        "apparmor=unconfined".into(),
        "--add-host".into(),
        "host.docker.internal:host-gateway".into(),
        "-e".into(),
        format!("HOST_PROJECT_DIR={}", project_dir.display()),
        "-e".into(),
        format!("MAUDEBOX_INSTANCE={instance_name}"),
    ];
    args.extend(network_args);
    args.extend(user_env);
    args.extend(term_env);
    args.extend(overlay_env);
    args.extend(aliases_env);
    args.extend(label_args);
    args.extend(overlay_args);
    args.extend(ephemeral_mount);
    args.extend(project_mounts);
    args.extend(claude_mounts);
    args.extend(managed_mcp_mount);
    args.extend(vcs_config_mounts);
    args.extend(extra_mount_args);
    args.push(opts.image.clone());
    args.extend(command);

    // When launched inside tmux, name the window after the instance so the
    // status bar / iTerm2 tmux tab tracks the feature for the container's
    // lifetime. The guard restores the prior window name on drop (after docker
    // exits). No-op outside tmux.
    let _tmux_window = crate::tmux::WindowName::apply(&instance_name);

    docker::run_inherit(&args)
}

// Build the instance handle from a project basename and the `--instance`
// discriminator: the bare basename when no discriminator was given, else
// `<basename>-<discriminator>`. Used both here (container name / label /
// `$MAUDEBOX_INSTANCE`) and by `list` to reconstruct the handle for a stopped
// `maudebox new` instance from its manifest.
pub fn instance_handle(basename: &str, discriminator: &str) -> String {
    if discriminator.is_empty() {
        basename.to_string()
    } else {
        format!("{basename}-{discriminator}")
    }
}

// Bash aliases only fire for interactive shells, so a non-interactive
// `docker run … <cmd>` would just exec the literal command. If the first
// remaining arg matches a configured alias name, rewrite
//   <name> <args>
// as
//   bash -c '<alias-value> "$@"' <name> <args>
// so the alias body runs through container bash (expanding $MAUDEBOX_INSTANCE
// and any other $VAR references at invocation time) with the trailing args
// appended via "$@".
fn resolve_alias(command: Vec<String>) -> Result<Vec<String>> {
    let Some(first) = command.first() else {
        return Ok(command);
    };
    if !is_valid_alias_name(first) {
        return Ok(command);
    }
    let aliases = read_aliases()?;
    let value = aliases.iter().find(|(n, _)| n == first).map(|(_, v)| v.clone());
    let Some(value) = value else {
        return Ok(command);
    };
    let mut out = vec![
        "bash".to_string(),
        "-c".to_string(),
        format!("{value} \"$@\""),
        first.clone(),
    ];
    out.extend(command.into_iter().skip(1));
    Ok(out)
}

fn is_valid_alias_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// Serialize `[mcp.NAME]` tables to `{"mcpServers": {...}}` JSON and write
// atomically to a stable host path. Docker captures the file inode at bind-
// mount time, so concurrent maudebox launches with different configs each see
// their own snapshot — atomic rename keeps any already-running container
// pointing at its original inode.
fn write_managed_mcp_json(
    servers: &std::collections::BTreeMap<String, toml::Value>,
) -> Result<PathBuf> {
    let dir = xdg_state_home()?.join("maudebox");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("managed-mcp.json");

    // toml::Value: Serialize and serde_json::Value: Deserialize, so the
    // round-trip lifts TOML straight to JSON. No datetime support is needed
    // — MCP server configs are strings/arrays/objects.
    let mut root = serde_json::Map::new();
    let mut inner = serde_json::Map::new();
    for (name, val) in servers {
        let json_val: serde_json::Value = serde_json::to_value(val)
            .with_context(|| format!("converting mcp.{name} to JSON"))?;
        inner.insert(name.clone(), json_val);
    }
    root.insert("mcpServers".to_string(), serde_json::Value::Object(inner));
    let content = serde_json::to_string_pretty(&serde_json::Value::Object(root))?;

    let tmp = dir.join(format!("managed-mcp.json.tmp.{}", std::process::id()));
    fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
    Ok(path)
}

// Encode a host path into Claude's auto-memory directory key: replace '/' and
// '.' with '-'. Inside the container the project is bind-mounted at its
// host path, so cwd canonicalizes the same on both sides — host and
// container agree on the key.
fn encode_project_key(p: &str) -> String {
    p.replace('/', "-").replace('.', "-")
}

#[cfg(unix)]
fn get_uid() -> u32 {
    // Safe FFI: getuid is always-succeed and re-entrant.
    unsafe { libc_getuid() }
}
#[cfg(unix)]
fn get_gid() -> u32 {
    unsafe { libc_getgid() }
}
#[cfg(not(unix))]
fn get_uid() -> u32 {
    0
}
#[cfg(not(unix))]
fn get_gid() -> u32 {
    0
}

// Avoid the `libc` crate just for getuid/getgid — these have a stable ABI on
// every unix and the FFI declarations are five lines.
#[cfg(unix)]
extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
    #[link_name = "getgid"]
    fn libc_getgid() -> u32;
}

