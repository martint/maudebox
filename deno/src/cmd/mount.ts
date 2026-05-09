import { mountSpecKey, parseMountSpec } from "../lib/mount.ts";
import { readMounts, writeMounts } from "../lib/config.ts";

// Manage the mounts list in the user config. Reads via the TOML parser;
// writes preserve every other line of the file (comments, other tables).
export async function cmdMount(args: string[]): Promise<number> {
    if (args[0] === "-h" || args[0] === "--help") {
        const { USAGE } = await import("../usage.ts");
        console.log(USAGE);
        return 0;
    }
    const action = args[0];
    if (!action) {
        console.error("maudebox mount: action required (add|list|rm)");
        return 1;
    }
    const rest = args.slice(1);
    const current = await readMounts();

    switch (action) {
        case "list":
            for (const s of current) console.log(s);
            return 0;

        case "add": {
            const spec = rest[0];
            if (!spec) {
                console.error("maudebox mount add: SPEC required");
                return 1;
            }
            try {
                parseMountSpec(spec);
            } catch (e) {
                console.error((e as Error).message);
                return 1;
            }
            const newKey = mountSpecKey(spec);
            const dup = current.find((s) => mountSpecKey(s) === newKey);
            if (dup) {
                console.log(`Already in the user config: ${dup}`);
                return 0;
            }
            await writeMounts([...current, spec]);
            console.log(`Added: ${spec}`);
            return 0;
        }

        case "rm": {
            const spec = rest[0];
            if (!spec) {
                console.error("maudebox mount rm: SPEC required");
                return 1;
            }
            const targetKey = mountSpecKey(spec);
            const idx = current.findIndex((s) => mountSpecKey(s) === targetKey);
            if (idx === -1) {
                console.error(`Not in the user config: ${spec}`);
                return 1;
            }
            const kept = current.slice(0, idx).concat(current.slice(idx + 1));
            await writeMounts(kept);
            console.log(`Removed: ${spec}`);
            return 0;
        }

        default:
            console.error(`maudebox mount: unknown action '${action}' (expected add|list|rm)`);
            return 1;
    }
}
