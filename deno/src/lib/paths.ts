import { join, resolve } from "@std/path";

export const CONTAINER_HOME = "/root";
export const IMAGE_NAME_DEFAULT = "maudebox";
export const STATE_VOLUME = "maudebox-state";

export function home(): string {
    const h = Deno.env.get("HOME");
    if (!h) throw new Error("HOME is not set");
    return h;
}

export function xdgConfigHome(): string {
    return Deno.env.get("XDG_CONFIG_HOME") ?? join(home(), ".config");
}

export function xdgStateHome(): string {
    return Deno.env.get("XDG_STATE_HOME") ?? join(home(), ".local", "state");
}

export function configPath(): string {
    return join(xdgConfigHome(), "maudebox", "config.toml");
}

// Bash `realpath` dereferences symlinks AND tolerates missing paths on most
// distros' default settings; Deno.realPath dereferences but throws on missing.
// This matches the bash script's effective behavior so volume-name hashes
// stay consistent for symlinked project paths.
export async function canonicalize(p: string): Promise<string> {
    try {
        return await Deno.realPath(p);
    } catch {
        return resolve(p);
    }
}

// Expand a leading `~` on the host side of a mount spec.
export function expandHostTilde(p: string): string {
    return p.startsWith("~") ? home() + p.slice(1) : p;
}

// Expand a leading `~` on the container side of a mount spec.
export function expandContainerTilde(p: string): string {
    return p.startsWith("~") ? CONTAINER_HOME + p.slice(1) : p;
}
