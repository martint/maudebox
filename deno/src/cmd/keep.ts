import { join } from "@std/path";
import { dockerCapture } from "../lib/docker.ts";

// Find a running ephemeral container by container ID, instance basename, or
// the original `maudebox new <name>` name, then drop the `keep` flag in its
// state dir. A no-op for non-ephemeral instances.
export async function cmdKeep(args: string[]): Promise<number> {
    if (args[0] === "-h" || args[0] === "--help") {
        const { USAGE } = await import("../usage.ts");
        console.log(USAGE);
        return 0;
    }
    const target = args[0];
    if (!target) {
        console.error("maudebox keep: <id-or-name> required");
        return 1;
    }

    const fmt = [
        "{{.ID}}",
        `{{.Label "maudebox.instance"}}`,
        `{{.Label "maudebox.ephemeral-name"}}`,
        `{{.Label "maudebox.ephemeral"}}`,
        `{{.Label "maudebox.state-dir"}}`,
        `{{.Label "maudebox.project"}}`,
    ].join("\t");

    const out = await dockerCapture([
        "ps",
        "--filter",
        "label=maudebox.instance",
        "--format",
        fmt,
    ]);

    type Match = {
        id: string;
        instance: string;
        ename: string;
        ephemeral: string;
        stateDir: string;
        project: string;
    };
    const matches: Match[] = out
        .split("\n")
        .filter((l) => l.length > 0)
        .map((l): Match => {
            const [id, instance, ename, ephemeral, stateDir, project] = l.split("\t");
            return {
                id: id ?? "",
                instance: instance ?? "",
                ename: ename ?? "",
                ephemeral: ephemeral ?? "",
                stateDir: stateDir ?? "",
                project: project ?? "",
            };
        })
        .filter((m) => m.id === target || m.instance === target || m.ename === target);

    if (matches.length === 0) {
        console.error(`No running maudebox container matches '${target}'.`);
        return 1;
    }
    if (matches.length > 1) {
        console.error(`Multiple running maudebox containers match '${target}':`);
        for (const m of matches) {
            console.error(`  ${m.id}  ${m.instance}  (${m.project})`);
        }
        return 1;
    }

    const m = matches[0]!;
    if (m.ephemeral !== "true") {
        console.log(`Instance '${m.instance}' is not ephemeral; nothing to keep.`);
        return 0;
    }
    if (!m.stateDir) {
        console.error(
            `Instance '${m.instance}' was launched by an older version of maudebox that doesn't support 'keep'; restart the container to enable it.`,
        );
        return 1;
    }

    await Deno.mkdir(m.stateDir, { recursive: true });
    await Deno.writeTextFile(join(m.stateDir, "keep"), "");
    console.log(
        `Marked '${m.instance}' to keep — workspace and overlay volume will survive when the container exits.`,
    );
    return 0;
}
