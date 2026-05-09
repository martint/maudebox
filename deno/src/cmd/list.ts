import { exists } from "@std/fs";
import { join } from "@std/path";
import { dockerCapture } from "../lib/docker.ts";
import { computeStateDir } from "../lib/volume.ts";

type Row = {
    project: string;
    instance: string;
    ephemeral: string;
    id: string;
    overlays: number;
};

// An "instance" is a project: anything currently running OR with at least
// one overlay volume on disk shows up. Volume labels are authoritative when
// available — they survive container exit, so a stopped instance still has
// full metadata. Container labels fill in when there's no volume yet (a
// running instance with no overlay configured) and supply the running
// container ID.
//
// An ephemeral instance whose keep flag is set is shown as non-ephemeral,
// since at that point the workspace will outlive the container.
export async function cmdList(args: string[]): Promise<number> {
    if (args[0] === "-h" || args[0] === "--help") {
        const { USAGE } = await import("../usage.ts");
        console.log(USAGE);
        return 0;
    }

    const volFmt = `{{.Label "maudebox.project"}}\t{{.Label "maudebox.instance"}}\t{{.Label "maudebox.ephemeral"}}`;
    const psFmt = `{{.Label "maudebox.project"}}\t{{.ID}}\t{{.Label "maudebox.instance"}}\t{{.Label "maudebox.ephemeral"}}`;

    const psOut = await dockerCapture([
        "ps",
        "--filter",
        "label=maudebox.instance",
        "--format",
        psFmt,
    ]);
    const volOut = await dockerCapture([
        "volume",
        "ls",
        "--filter",
        "name=maudebox-overlay-",
        "--format",
        volFmt,
    ]);

    const volumes = volOut.split("\n").filter((l) => l.length > 0).map((l) => {
        const [project, instance, ephemeral] = l.split("\t");
        return {
            project: project ?? "",
            instance: instance ?? "",
            ephemeral: ephemeral ?? "",
        };
    });
    const running = psOut.split("\n").filter((l) => l.length > 0).map((l) => {
        const [project, id, instance, ephemeral] = l.split("\t");
        return {
            project: project ?? "",
            id: id ?? "",
            instance: instance ?? "",
            ephemeral: ephemeral ?? "",
        };
    });

    const projects = new Set<string>();
    for (const v of volumes) if (v.project) projects.add(v.project);
    for (const r of running) if (r.project) projects.add(r.project);

    if (projects.size === 0) {
        console.log("No maudebox instances found.");
        return 0;
    }

    const rows: Row[] = [];
    for (const project of [...projects].sort()) {
        const vol = volumes.find((v) => v.project === project);
        const run = running.find((r) => r.project === project);
        const instance = vol?.instance || run?.instance || "";
        let ephemeral = vol?.ephemeral || run?.ephemeral || "";
        const id = run?.id ?? "";
        const overlays = volumes.filter((v) => v.project === project).length;

        if (ephemeral === "true") {
            const stateDir = computeStateDir(project);
            if (await exists(join(stateDir, "keep"))) ephemeral = "false";
        }

        rows.push({ project, instance, ephemeral, id, overlays });
    }

    const header = ["ID", "STATUS", "INSTANCE", "EPHEMERAL", "OVERLAYS", "PATH"];
    const data = rows.map((r) => [
        r.id || "-",
        r.id ? "running" : "stopped",
        r.instance || "?",
        r.ephemeral || "-",
        String(r.overlays),
        r.project,
    ]);
    printTable([header, ...data]);
    return 0;
}

// Two-space column padding, matching `column -t`.
function printTable(rows: readonly (readonly string[])[]): void {
    if (rows.length === 0) return;
    const cols = rows[0]!.length;
    const widths = new Array(cols).fill(0);
    for (const row of rows) {
        for (let i = 0; i < cols; i++) {
            widths[i] = Math.max(widths[i], (row[i] ?? "").length);
        }
    }
    for (const row of rows) {
        const parts = row.map((c, i) => (i < cols - 1 ? c.padEnd(widths[i]) : c));
        console.log(parts.join("  "));
    }
}
