import { dirname } from "@std/path";
import { parse as parseToml } from "@std/toml";
import { configPath } from "./paths.ts";

// ── reads ────────────────────────────────────────────────────────────────────
// We use a real TOML parser on the read side (improvement over the bash
// version's hand-rolled subset) but keep the comment-preserving line-
// streaming approach on the write side so user comments don't disappear.

export async function readConfig(): Promise<Record<string, unknown>> {
    let text: string;
    try {
        text = await Deno.readTextFile(configPath());
    } catch (e) {
        if (e instanceof Deno.errors.NotFound) return {};
        throw e;
    }
    return parseToml(text) as Record<string, unknown>;
}

export async function readMounts(): Promise<string[]> {
    const cfg = await readConfig();
    const m = cfg.mounts;
    if (!Array.isArray(m)) return [];
    return m.filter((x): x is string => typeof x === "string");
}

export async function readAliases(): Promise<Map<string, string>> {
    const cfg = await readConfig();
    const a = cfg.aliases;
    const out = new Map<string, string>();
    if (a && typeof a === "object" && !Array.isArray(a)) {
        for (const [k, v] of Object.entries(a as Record<string, unknown>)) {
            if (typeof v === "string") out.set(k, v);
        }
    }
    return out;
}

// ── writes (comment-preserving) ──────────────────────────────────────────────

function emitMountsBlock(specs: readonly string[]): string {
    if (specs.length === 0) return "mounts = []\n";
    return "mounts = [\n" + specs.map((s) => `    "${s}",\n`).join("") + "]\n";
}

function escapeTomlString(s: string): string {
    return s.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
}

function emitAliasesBlock(entries: ReadonlyMap<string, string>): string {
    let out = "[aliases]\n";
    for (const [name, value] of entries) {
        out += `${name} = "${escapeTomlString(value)}"\n`;
    }
    return out;
}

async function writeAtomic(path: string, content: string): Promise<void> {
    await Deno.mkdir(dirname(path), { recursive: true });
    const tmp = `${path}.tmp.${Deno.pid}`;
    await Deno.writeTextFile(tmp, content);
    await Deno.rename(tmp, path);
}

// Replace just the `mounts = [...]` block, keep every other line of the file
// (comments, other TOML keys, blanks) untouched. Multi-line array support:
// once we hit `mounts = [`, swallow input lines until the closing `]`. If
// no mounts assignment exists at all, append at the end.
export async function writeMounts(specs: readonly string[]): Promise<void> {
    const path = configPath();
    let existing: string | null = null;
    try {
        existing = await Deno.readTextFile(path);
    } catch (e) {
        if (!(e instanceof Deno.errors.NotFound)) throw e;
    }

    if (existing === null) {
        await writeAtomic(path, emitMountsBlock(specs));
        return;
    }

    const lines = existing.split("\n");
    // split() on a trailing newline yields a final empty element; drop it
    // and we'll re-add the trailing newline at the end.
    const hadTrailingNewline = existing.endsWith("\n");
    if (hadTrailingNewline) lines.pop();

    let out = "";
    let inBlock = false;
    let replaced = false;
    for (const line of lines) {
        if (!inBlock && /^\s*mounts\s*=\s*\[/.test(line)) {
            out += emitMountsBlock(specs);
            replaced = true;
            if (line.includes("]")) continue; // single-line array: done
            inBlock = true;
            continue;
        }
        if (inBlock) {
            if (line.includes("]")) inBlock = false;
            continue;
        }
        out += line + "\n";
    }
    if (!replaced) out += emitMountsBlock(specs);
    await writeAtomic(path, out);
}

// Replace the `[aliases]` table. The section ends at the next `[section]`
// header (which we then need to print since it belongs to the surrounding
// content) or at EOF.
export async function writeAliases(entries: ReadonlyMap<string, string>): Promise<void> {
    const path = configPath();
    let existing: string | null = null;
    try {
        existing = await Deno.readTextFile(path);
    } catch (e) {
        if (!(e instanceof Deno.errors.NotFound)) throw e;
    }

    if (existing === null) {
        await writeAtomic(path, emitAliasesBlock(entries));
        return;
    }

    const lines = existing.split("\n");
    if (existing.endsWith("\n")) lines.pop();

    let out = "";
    let inSection = false;
    let replaced = false;
    for (const line of lines) {
        if (!inSection && /^\s*\[aliases\]\s*$/.test(line)) {
            out += emitAliasesBlock(entries);
            replaced = true;
            inSection = true;
            continue;
        }
        if (inSection) {
            if (/^\s*\[/.test(line)) {
                inSection = false;
                out += line + "\n";
            }
            continue;
        }
        out += line + "\n";
    }
    if (!replaced) out += emitAliasesBlock(entries);
    await writeAtomic(path, out);
}
