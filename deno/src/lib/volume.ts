import { basename, join } from "@std/path";
import { createHash } from "node:crypto";
import { xdgStateHome } from "./paths.ts";

// Bash `echo "$x" | sha256sum` includes the trailing newline echo adds —
// matching it byte-for-byte is what keeps the deno port's volume names
// compatible with volumes created by the bash version.
export function sha256Prefix(input: string): string {
    return createHash("sha256").update(input + "\n").digest("hex").slice(0, 8);
}

// Project key: human-readable basename + 8-char hash of the full path. Used
// as the prefix for every overlay volume on a given project and as the
// state-dir name. Collisions on basename alone aren't enough to share a
// scope (two different /a/myproj and /b/myproj must be distinct).
export function computeVolumeName(projectDir: string): string {
    return `maudebox-overlay-${basename(projectDir)}-${sha256Prefix(projectDir)}`;
}

// Per-overlay volume name: project key + 8-char hash of the container target,
// so a single project can have multiple overlay mounts (e.g. ~/.m2 and
// ~/.cargo) without their volumes colliding.
export function computeOverlayVolume(projectDir: string, containerTarget: string): string {
    return `${computeVolumeName(projectDir)}-${sha256Prefix(containerTarget)}`;
}

// Per-instance state dir on the host. Bind-mounted at /run/maudebox inside
// the container for ephemeral instances so `maudebox-keep` (or host-side
// `maudebox keep`) can drop a flag the cleanup trap reads after exit.
export function computeStateDir(projectDir: string): string {
    return join(xdgStateHome(), "maudebox", "instances", computeVolumeName(projectDir));
}
