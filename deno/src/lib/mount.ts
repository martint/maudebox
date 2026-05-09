import { expandContainerTilde, expandHostTilde } from "./paths.ts";

export type MountMode = "ro" | "rw" | "overlay";

export type MountSpec = {
    src: string;
    dst: string;
    mode: MountMode;
};

// Parse a HOST:CONTAINER[:MODE] spec and validate each field. Empty src/dst,
// an unknown mode, or extra ':'-separated fields would otherwise fall
// through to docker (or be silently persisted by `mount add`) and the
// failure would surface far from the typo.
export function parseMountSpec(spec: string): MountSpec {
    const parts = spec.split(":");
    if (parts.length < 2 || parts.length > 3) {
        throw new Error(
            parts.length > 3
                ? `Invalid mount spec: '${spec}' (too many ':'-separated fields)`
                : `Invalid mount spec: '${spec}' (expected HOST:CONTAINER[:ro|rw|overlay])`,
        );
    }
    const [src, dst, modeRaw] = parts;
    if (!src || !dst) {
        throw new Error(
            `Invalid mount spec: '${spec}' (expected HOST:CONTAINER[:ro|rw|overlay])`,
        );
    }
    const mode = (modeRaw ?? "rw") as string;
    if (mode !== "ro" && mode !== "rw" && mode !== "overlay") {
        throw new Error(
            `Invalid mount mode: '${mode}' in '${spec}' (expected ro, rw, or overlay)`,
        );
    }
    return { src, dst, mode };
}

// Canonical key for spec equality. Two specs that mean the same thing should
// produce the same key — so `~/x:~/x` (mode defaults to rw) and `~/x:~/x:rw`
// collide on add/rm. Malformed specs round-trip as themselves; that's fine
// for comparison (won't match anything well-formed; add validates separately).
export function mountSpecKey(spec: string): string {
    try {
        const { src, dst, mode } = parseMountSpec(spec);
        return `${src}:${dst}:${mode}`;
    } catch {
        return spec;
    }
}

export type Overlay = { hostSrc: string; containerDst: string };

// Result of resolving every mount spec in a launch: the docker `-v` args for
// rw/ro mounts, plus an ordered list of overlay mounts that the launch step
// materializes into per-project Docker volumes.
export type MountPlan = {
    mountArgs: string[];
    overlays: Overlay[];
};

export function buildMountPlan(specs: readonly string[]): MountPlan {
    const mountArgs: string[] = [];
    const overlays: Overlay[] = [];

    for (const spec of specs) {
        const { src: srcRaw, dst: dstRaw, mode } = parseMountSpec(spec);
        const src = expandHostTilde(srcRaw);
        const dst = expandContainerTilde(dstRaw);

        if (mode === "overlay") {
            const dup = overlays.find((o) => o.containerDst === dst);
            if (dup) {
                throw new Error(
                    `Duplicate overlay target: ${dst} (sources: ${dup.hostSrc} and ${src})`,
                );
            }
            overlays.push({ hostSrc: src, containerDst: dst });
            continue;
        }
        mountArgs.push("-v", `${src}:${dst}:${mode}`);
    }

    return { mountArgs, overlays };
}
