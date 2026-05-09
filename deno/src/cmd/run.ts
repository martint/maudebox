import { exists } from "@std/fs";
import { basename, join } from "@std/path";
import { dockerExec, ensureSubpaths, ensureVolume } from "../lib/docker.ts";
import { canonicalize, home, STATE_VOLUME } from "../lib/paths.ts";
import { buildMountPlan, type Overlay } from "../lib/mount.ts";
import { computeOverlayVolume, computeStateDir } from "../lib/volume.ts";
import { readAliases, readMounts } from "../lib/config.ts";
import { detectVcsBase } from "../lib/vcs.ts";

export type RunOptions = {
    image: string;
    memoryFrom: string;
    extraMounts: string[];
    ephemeralName: string;
    projectDir: string;
    command: string[];
};

const NAME_RE = /^[a-zA-Z_][a-zA-Z0-9_-]*$/;

export async function cmdRun(opts: RunOptions): Promise<number> {
    const { image, memoryFrom, ephemeralName } = opts;
    const projectDir = await canonicalize(opts.projectDir);

    if (!(await isDir(projectDir))) {
        console.error(`Not a directory: ${projectDir}`);
        return 1;
    }

    const command = await resolveAlias(opts.command);
    const vcsBase = await detectVcsBase(projectDir);

    // ── persistent state (maudebox-state volume + read-only host overlays) ──
    await ensureVolume(STATE_VOLUME);
    await ensureSubpaths(image, STATE_VOLUME, ["claude", "gh"]);

    const claudeMounts: string[] = [
        "--mount",
        `type=volume,src=${STATE_VOLUME},dst=/root/.claude,volume-subpath=claude`,
        "--mount",
        `type=volume,src=${STATE_VOLUME},dst=/root/.config/gh,volume-subpath=gh`,
    ];
    const hostClaudeDir = join(home(), ".claude");
    for (const p of ["CLAUDE.md", "settings.json", "agents", "commands", "plugins"]) {
        if (await exists(join(hostClaudeDir, p))) {
            claudeMounts.push("-v", `${hostClaudeDir}/${p}:/root/.claude/${p}:ro`);
        }
    }

    // Auto-memory: bind-mount the host's projects/<key>/memory rw so memories
    // round-trip between host and container. --memory-from breaks the host/
    // container symmetry: container target is keyed by projectDir (Claude Code
    // derives it from cwd inside the container) but the host source can be
    // keyed elsewhere, so an ephemeral workspace can share its parent project's
    // memory.
    const projectKey = encodeProjectKey(projectDir);
    const memorySource = memoryFrom || projectDir;
    const memorySourceKey = encodeProjectKey(memorySource);
    const hostMemoryDir = join(hostClaudeDir, "projects", memorySourceKey, "memory");
    await Deno.mkdir(hostMemoryDir, { recursive: true });
    claudeMounts.push(
        "-v",
        `${hostMemoryDir}:/root/.claude/projects/${projectKey}/memory`,
    );

    // ── host VCS config (read-only) ────────────────────────────────────────
    const vcsConfigMounts: string[] = [];
    const gitconfig = join(home(), ".gitconfig");
    const gitConfigDir = join(home(), ".config", "git");
    const jjConfig = join(home(), ".config", "jj", "config.toml");
    if (await exists(gitconfig)) {
        vcsConfigMounts.push("-v", `${gitconfig}:/root/.gitconfig:ro`);
    }
    if (await exists(gitConfigDir)) {
        vcsConfigMounts.push("-v", `${gitConfigDir}:/root/.config/git:ro`);
    }
    if (await exists(jjConfig)) {
        vcsConfigMounts.push("-v", `${jjConfig}:/root/.config/jj/config.toml:ro`);
    }

    // ── extra mounts (CLI --mount + config-file mounts) ────────────────────
    const allMountSpecs = [...opts.extraMounts, ...(await readMounts())];
    const { mountArgs: extraMountArgs, overlays } = buildMountPlan(allMountSpecs);

    // ── aliases env var (parsed by /etc/profile.d/maudebox-aliases.sh) ─────
    const aliasesEnv: string[] = [];
    const aliases = await readAliases();
    if (aliases.size > 0) {
        let val = "";
        for (const [name, value] of aliases) val += `${name}=${value}\n`;
        aliasesEnv.push("-e", `MAUDEBOX_ALIASES=${val}`);
    }

    // ── labels (instance/project/ephemeral) ────────────────────────────────
    const instanceName = basename(projectDir);
    const labels: string[] = [
        "--label",
        `maudebox.instance=${instanceName}`,
        "--label",
        `maudebox.project=${projectDir}`,
    ];
    const ephemeralMount: string[] = [];
    if (ephemeralName) {
        const stateDir = computeStateDir(projectDir);
        await Deno.mkdir(stateDir, { recursive: true });
        labels.push(
            "--label",
            "maudebox.ephemeral=true",
            "--label",
            `maudebox.ephemeral-name=${ephemeralName}`,
            "--label",
            `maudebox.state-dir=${stateDir}`,
        );
        ephemeralMount.push("-v", `${stateDir}:/run/maudebox`);
    } else {
        labels.push("--label", "maudebox.ephemeral=false");
    }

    // ── overlay volumes + entrypoint env ───────────────────────────────────
    const overlayArgs: string[] = [];
    const overlayEnv: string[] = [];
    if (overlays.length > 0) {
        let envVal = "";
        for (let i = 0; i < overlays.length; i++) {
            const o = overlays[i] as Overlay;
            const n = i + 1;
            const vol = computeOverlayVolume(projectDir, o.containerDst);
            await ensureVolume(vol, [
                ...labelsToFlat(labels),
                `maudebox.overlay-target=${o.containerDst}`,
            ]);
            overlayArgs.push(
                "-v",
                `${o.hostSrc}:/maudebox/overlay-${n}/lower:ro`,
                "-v",
                `${vol}:/maudebox/overlay-${n}/upper`,
            );
            envVal += `/maudebox/overlay-${n}/lower:/maudebox/overlay-${n}/upper:${o.containerDst}\n`;
        }
        overlayEnv.push("-e", `MAUDEBOX_OVERLAYS=${envVal}`);
    }

    // ── project + base-repo mounts (host paths preserved) ──────────────────
    const projectMounts: string[] = ["-v", `${projectDir}:${projectDir}`];
    if (vcsBase && vcsBase !== projectDir) {
        projectMounts.push("-v", `${vcsBase}:${vcsBase}`);
    }

    // ── env: UID/GID on Linux, terminal forwarding ─────────────────────────
    const userEnv: string[] = [];
    if (Deno.build.os === "linux") {
        userEnv.push("-e", `HOST_UID=${Deno.uid() ?? 0}`, "-e", `HOST_GID=${Deno.gid() ?? 0}`);
    }
    const termEnv: string[] = ["-e", "TERM_PROGRAM=tmux"];
    for (const v of ["COLORTERM", "LC_TERMINAL", "LC_TERMINAL_VERSION"]) {
        const val = Deno.env.get(v);
        if (val) termEnv.push("-e", `${v}=${val}`);
    }

    // ── status preamble ────────────────────────────────────────────────────
    console.log(`Image    : ${image}`);
    console.log(`Worktree : ${projectDir}`);
    if (vcsBase && vcsBase !== projectDir) console.log(`VCS base : ${vcsBase}`);
    for (const o of overlays) console.log(`Overlay  : ${o.hostSrc} -> ${o.containerDst}`);

    // ── exec docker run ────────────────────────────────────────────────────
    const args = [
        "run",
        "--rm",
        "-it",
        "--cap-add",
        "SYS_ADMIN",
        "--security-opt",
        "apparmor=unconfined",
        "-e",
        `HOST_PROJECT_DIR=${projectDir}`,
        "-e",
        `MAUDEBOX_INSTANCE=${instanceName}`,
        ...userEnv,
        ...termEnv,
        ...overlayEnv,
        ...aliasesEnv,
        ...labels,
        ...overlayArgs,
        ...ephemeralMount,
        ...projectMounts,
        ...claudeMounts,
        ...vcsConfigMounts,
        ...extraMountArgs,
        image,
        ...command,
    ];
    return await dockerExec(args);
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
async function resolveAlias(command: string[]): Promise<string[]> {
    if (command.length === 0) return command;
    const first = command[0]!;
    if (!NAME_RE.test(first)) return command;
    const aliases = await readAliases();
    const value = aliases.get(first);
    if (value === undefined) return command;
    return ["bash", "-c", `${value} "$@"`, first, ...command.slice(1)];
}

// Encode a host path into Claude's auto-memory directory key: replace '/'
// and '.' with '-'. Inside the container the project is bind-mounted at its
// host path, so cwd canonicalizes the same on both sides — host and container
// agree on the key.
function encodeProjectKey(p: string): string {
    return p.replaceAll("/", "-").replaceAll(".", "-");
}

async function isDir(p: string): Promise<boolean> {
    try {
        return (await Deno.stat(p)).isDirectory;
    } catch {
        return false;
    }
}

// `--label foo=bar --label baz=qux` → `["foo=bar", "baz=qux"]`
function labelsToFlat(labelArgs: readonly string[]): string[] {
    const out: string[] = [];
    for (let i = 0; i < labelArgs.length; i += 2) {
        if (labelArgs[i] === "--label" && labelArgs[i + 1]) {
            out.push(labelArgs[i + 1] as string);
        }
    }
    return out;
}
