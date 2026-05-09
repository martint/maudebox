import { dockerCapture } from "../lib/docker.ts";
import { canonicalize } from "../lib/paths.ts";

// Remove every overlay volume tied to a project. Volumes carry the
// maudebox.project label as the canonical link (more durable than the
// volume's name suffix scheme). Doesn't validate that the project still
// exists — cleaning orphan volumes for a since-deleted worktree by
// passing the original path is a legitimate use.
export async function cmdClean(args: string[]): Promise<number> {
    if (args[0] === "-h" || args[0] === "--help") {
        const { USAGE } = await import("../usage.ts");
        console.log(USAGE);
        return 0;
    }
    const projectDir = await canonicalize(args[0] ?? ".");
    const out = await dockerCapture([
        "volume",
        "ls",
        "--filter",
        `label=maudebox.project=${projectDir}`,
        "--format",
        "{{.Name}}",
    ]);
    const volumes = out.split("\n").filter((v) => v.length > 0);
    if (volumes.length === 0) {
        console.log(`No overlay volumes found for: ${projectDir}`);
        return 0;
    }
    for (const v of volumes) {
        await dockerCapture(["volume", "rm", v]);
        console.log(`Removed volume: ${v}`);
    }
    return 0;
}
