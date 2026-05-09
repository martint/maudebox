import { join, resolve } from "@std/path";

// A jj workspace stores the absolute or relative path to the *main* repo's
// .jj/repo directory inside <workspace>/.jj/repo (a regular file). A git
// worktree stores `gitdir: <abs-path>` in <worktree>/.git (also a regular
// file). In both cases the worktree's metadata references an absolute host
// path that won't exist inside the container unless we bind-mount the base
// repo at the same path. This returns that base path (the main repo's
// working tree, not its .jj/.git dir) or null if the project isn't a
// worktree/workspace.
export async function detectVcsBase(dir: string): Promise<string | null> {
    if (!(await isDirectory(dir))) return null;

    const jjBase = await detectJjBase(dir);
    if (jjBase) return jjBase;

    return await detectGitBase(dir);
}

async function detectJjBase(dir: string): Promise<string | null> {
    const repoFile = join(dir, ".jj", "repo");
    if (!(await isFile(repoFile))) return null;

    const raw = (await Deno.readTextFile(repoFile)).replace(/\n$/, "");
    const target = raw.startsWith("/") ? raw : await safeRealPath(join(dir, ".jj", raw));
    if (!target) return null;

    // target is <main>/.jj/repo; the working tree is two levels up.
    const base = target.endsWith("/.jj/repo") ? target.slice(0, -"/.jj/repo".length) : target;
    if (base === target || base === dir || base === "") return null;
    return base;
}

async function detectGitBase(dir: string): Promise<string | null> {
    const gitFile = join(dir, ".git");
    if (!(await isFile(gitFile))) return null; // dirs (regular checkouts) don't need rewiring

    const text = await Deno.readTextFile(gitFile);
    const match = text.split("\n").find((l) => l.startsWith("gitdir:"));
    if (!match) return null;
    let gitdir = match.replace(/^gitdir:\s*/, "").trim();
    if (!gitdir.startsWith("/")) gitdir = (await safeRealPath(resolve(dir, gitdir))) ?? "";
    if (!gitdir) return null;

    // gitdir is typically <main>/.git/worktrees/<name>, sometimes <main>/.git.
    let base = gitdir;
    const wtIdx = gitdir.indexOf("/.git/worktrees/");
    if (wtIdx >= 0) base = gitdir.slice(0, wtIdx);
    else if (gitdir.endsWith("/.git")) base = gitdir.slice(0, -"/.git".length);
    if (base === gitdir || base === dir || base === "") return null;
    return base;
}

async function isDirectory(p: string): Promise<boolean> {
    try {
        return (await Deno.stat(p)).isDirectory;
    } catch {
        return false;
    }
}

async function isFile(p: string): Promise<boolean> {
    try {
        return (await Deno.stat(p)).isFile;
    } catch {
        return false;
    }
}

async function safeRealPath(p: string): Promise<string | null> {
    try {
        return await Deno.realPath(p);
    } catch {
        return null;
    }
}
