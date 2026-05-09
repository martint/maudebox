// maudebox – launch a maudebox container for a given worktree
//
// Deno port of the bash wrapper. Same on-disk contract: identical volume
// names, label scheme, MAUDEBOX_OVERLAYS / MAUDEBOX_ALIASES env-var format,
// project-key encoding for the auto-memory dir, etc. — so it's a drop-in
// replacement that can coexist with bash-launched containers and volumes.

import { IMAGE_NAME_DEFAULT } from "./lib/paths.ts";
import { USAGE } from "./usage.ts";
import { cmdAlias } from "./cmd/alias.ts";
import { cmdClean } from "./cmd/clean.ts";
import { cmdKeep } from "./cmd/keep.ts";
import { cmdList } from "./cmd/list.ts";
import { cmdMount } from "./cmd/mount.ts";
import { cmdNew } from "./cmd/new.ts";
import { cmdRun } from "./cmd/run.ts";

const READONLY_SUBCOMMANDS = new Set(["list", "clean", "keep", "mount", "alias"]);

async function main(argv: string[]): Promise<number> {
    const first = argv[0];

    // Read-only subcommands (no container launch) get dispatched first, so
    // launch flags below can't silently apply to them.
    if (first === "-h" || first === "--help") {
        console.log(USAGE);
        return 0;
    }
    if (first && READONLY_SUBCOMMANDS.has(first)) {
        const rest = argv.slice(1);
        switch (first) {
            case "list":
                return await cmdList(rest);
            case "clean":
                return await cmdClean(rest);
            case "keep":
                return await cmdKeep(rest);
            case "mount":
                return await cmdMount(rest);
            case "alias":
                return await cmdAlias(rest);
        }
    }

    // Parse outer launch flags (everything starting with `--` before the
    // first positional). `--ephemeral-name` is internal: cmd_new passes it
    // on its recursive invocation to mark the inner run as ephemeral. Not
    // advertised in --help — but the deno port doesn't actually need it
    // since cmdNew calls cmdRun in-process. We still accept the flag so a
    // bash-version `maudebox` invoking `maudebox-deno` (or vice versa) keeps
    // working during a transition.
    let image = IMAGE_NAME_DEFAULT;
    let memoryFrom = "";
    let ephemeralName = "";
    const extraMounts: string[] = [];
    let i = 0;
    while (i < argv.length && argv[i]!.startsWith("--")) {
        const flag = argv[i]!;
        const val = argv[i + 1];
        switch (flag) {
            case "--tag":
                if (!val) return errMissingValue(flag);
                image = val;
                i += 2;
                break;
            case "--memory-from":
                if (!val) return errMissingValue(flag);
                memoryFrom = val;
                i += 2;
                break;
            case "--mount":
                if (!val) return errMissingValue(flag);
                extraMounts.push(val);
                i += 2;
                break;
            case "--ephemeral-name":
                if (!val) return errMissingValue(flag);
                ephemeralName = val;
                i += 2;
                break;
            default:
                console.error(`Unknown option: ${flag}`);
                console.error(USAGE);
                return 1;
        }
    }

    const rest = argv.slice(i);
    const next = rest[0];

    // `new` accepts the launch flags (forwards them to its recursive launch).
    if (next === "new") {
        return await cmdNew(rest.slice(1), { image, extraMounts });
    }

    // Catch `maudebox --tag foo list` (launch flags before a non-launch
    // subcommand) with a clear error rather than letting it fall through to
    // PROJECT_DIR validation as "Not a directory: ./list".
    if (next && READONLY_SUBCOMMANDS.has(next)) {
        console.error(
            `Error: '${next}' is a subcommand and must come first; --tag/--memory-from/--mount don't apply to it`,
        );
        return 1;
    }

    // Default: run a container against rest[0] (default cwd) with rest[1..]
    // as the inner command.
    const projectDir = next ?? ".";
    const command = rest.slice(next ? 1 : 0);
    return await cmdRun({
        image,
        memoryFrom,
        extraMounts,
        ephemeralName,
        projectDir,
        command,
    });
}

function errMissingValue(flag: string): number {
    console.error(`${flag}: missing value`);
    return 1;
}

if (import.meta.main) {
    try {
        Deno.exit(await main(Deno.args));
    } catch (e) {
        console.error((e as Error).message ?? String(e));
        Deno.exit(1);
    }
}
