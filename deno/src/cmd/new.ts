import { basename, dirname, isAbsolute, join, resolve } from "@std/path";
import { dockerCapture } from "../lib/docker.ts";
import { computeStateDir, computeVolumeName } from "../lib/volume.ts";
import { cmdRun } from "./run.ts";

// Outer flags forwarded by main.ts when the user types
//   maudebox [--tag T] [--mount S]... new ...
type OuterFlags = {
    image: string;
    extraMounts: string[];
};

// Create a jj workspace or git worktree from a source project and run
// maudebox on it. By default the workspace persists after the container
// exits, like `maudebox <path>` does. With --ephemeral, the workspace and
// its overlay volume are torn down on exit. jj is preferred when both .jj
// and .git are present (colocated repos).
export async function cmdNew(args: string[], outer: OuterFlags): Promise<number> {
    let target = "";
    let fromRev = "";
    let ephemeral = false;
    const positional: string[] = [];

    for (let i = 0; i < args.length; i++) {
        const a = args[i]!;
        if (a === "-h" || a === "--help") {
            const { USAGE } = await import("../usage.ts");
            console.log(USAGE);
            return 0;
        }
        if (a === "--path") {
            target = args[++i] ?? "";
            continue;
        }
        if (a === "--from") {
            fromRev = args[++i] ?? "";
            continue;
        }
        if (a === "--ephemeral") {
            ephemeral = true;
            continue;
        }
        if (a.startsWith("-")) {
            console.error(`Unknown option: ${a}`);
            return 1;
        }
        positional.push(a);
    }

    const name = positional[0];
    if (!name) {
        console.error("maudebox new: <name> is required");
        return 1;
    }

    // Positional shape after <name>:
    //   <source-dir> <command...>   if positional[1] exists as a directory
    //   <command...>                otherwise (source defaults to cwd)
    let source = ".";
    let innerCmd: string[] = [];
    if (positional.length >= 2 && (await isDir(positional[1]!))) {
        source = positional[1]!;
        innerCmd = positional.slice(2);
    } else if (positional.length >= 2) {
        innerCmd = positional.slice(1);
    }

    source = await Deno.realPath(source);
    if (!(await isDir(source))) {
        console.error(`Not a directory: ${source}`);
        return 1;
    }

    if (!target) {
        target = join(dirname(source), `${basename(source)}.${name}`);
    } else if (!isAbsolute(target)) {
        target = resolve(Deno.cwd(), target);
    }

    if (await pathExists(target)) {
        console.error(`Target path already exists: ${target}`);
        return 1;
    }

    let kind: "jj" | "git";
    if (await pathExists(join(source, ".jj"))) {
        kind = "jj";
        console.log(`Creating jj workspace '${name}' at: ${target}`);
        const jjArgs = ["workspace", "add", "--name", name];
        if (fromRev) jjArgs.push("-r", fromRev);
        jjArgs.push(target);
        await runIn(source, "jj", jjArgs);
    } else if (await pathExists(join(source, ".git"))) {
        kind = "git";
        const rev = fromRev || "HEAD";
        console.log(
            `Creating git worktree '${name}' at: ${target} (branch: ${name} from ${rev})`,
        );
        await runIn(source, "git", ["worktree", "add", "-b", name, target, rev]);
    } else {
        console.error(`Not a jj or git repo: ${source}`);
        return 1;
    }

    // Recursive launch — we just call cmdRun directly rather than spawning
    // another wrapper process. Same effect, fewer hops.
    let rc = 0;
    try {
        rc = await cmdRun({
            image: outer.image,
            memoryFrom: source,
            extraMounts: outer.extraMounts,
            ephemeralName: ephemeral ? name : "",
            projectDir: target,
            command: innerCmd,
        });
    } finally {
        if (ephemeral) {
            await ephemeralCleanup({ name, target, source, kind });
        }
    }
    return rc;
}

type CleanupArgs = { name: string; target: string; source: string; kind: "jj" | "git" };

async function ephemeralCleanup(c: CleanupArgs): Promise<void> {
    const stateDir = computeStateDir(c.target);

    // The user (in-container `maudebox-keep` or host `maudebox keep`) can drop
    // a `keep` file in the state dir to disarm cleanup mid-session. Honour it
    // by leaving the workspace and overlay in place — but still drop the now-
    // stale state dir so a future `maudebox` invocation against this same path
    // starts fresh (it's no longer ephemeral after this point).
    if (await pathExists(join(stateDir, "keep"))) {
        console.log(`Keep flag set; preserving ${c.kind} workspace at: ${c.target}`);
        await rmrf(stateDir);
        return;
    }

    console.log(`Cleaning up ephemeral ${c.kind} workspace at: ${c.target}`);
    if (c.kind === "jj") {
        await runIn(c.source, "jj", ["workspace", "forget", c.name], { ignoreFailure: true });
        await rmrf(c.target);
    } else {
        await runIn(c.source, "git", ["worktree", "remove", "--force", c.target], {
            ignoreFailure: true,
        });
        await runIn(c.source, "git", ["branch", "-D", c.name], { ignoreFailure: true });
    }
    const volume = computeVolumeName(c.target);
    try {
        await dockerCapture(["volume", "rm", volume]);
    } catch {
        // best-effort: a non-overlay run never created the volume
    }
    await rmrf(stateDir);
}

async function runIn(
    cwd: string,
    cmd: string,
    args: string[],
    opts: { ignoreFailure?: boolean } = {},
): Promise<void> {
    const c = new Deno.Command(cmd, {
        args,
        cwd,
        stdin: "inherit",
        stdout: "inherit",
        stderr: "inherit",
    });
    const { code } = await c.output();
    if (code !== 0 && !opts.ignoreFailure) {
        throw new Error(`${cmd} ${args.join(" ")} failed (exit ${code})`);
    }
}

async function isDir(p: string): Promise<boolean> {
    try {
        return (await Deno.stat(p)).isDirectory;
    } catch {
        return false;
    }
}

async function pathExists(p: string): Promise<boolean> {
    try {
        await Deno.lstat(p);
        return true;
    } catch {
        return false;
    }
}

async function rmrf(p: string): Promise<void> {
    try {
        await Deno.remove(p, { recursive: true });
    } catch {
        // best-effort cleanup
    }
}
