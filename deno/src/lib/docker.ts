// Thin wrappers around `docker` CLI. We deliberately stay close to the
// shell-out shape — the bash version's contract is "we drive the docker
// CLI", and the deno port keeps that for compatibility with existing
// volumes/labels.

export type RunOptions = {
    args: string[];
    stdin?: "inherit" | "null";
    stdout?: "inherit" | "piped" | "null";
    stderr?: "inherit" | "piped" | "null";
};

// Run `docker <args>`, return its exit code, optionally captured stdout/err.
export async function dockerRun(opts: RunOptions): Promise<{
    code: number;
    stdout: string;
    stderr: string;
}> {
    const cmd = new Deno.Command("docker", {
        args: opts.args,
        stdin: opts.stdin ?? "inherit",
        stdout: opts.stdout ?? "inherit",
        stderr: opts.stderr ?? "inherit",
    });
    const { code, stdout, stderr } = await cmd.output();
    const dec = new TextDecoder();
    return {
        code,
        stdout: opts.stdout === "piped" ? dec.decode(stdout) : "",
        stderr: opts.stderr === "piped" ? dec.decode(stderr) : "",
    };
}

// Run docker, capture stdout. Throws on non-zero exit.
export async function dockerCapture(args: string[]): Promise<string> {
    const r = await dockerRun({ args, stdout: "piped", stderr: "piped" });
    if (r.code !== 0) {
        throw new Error(`docker ${args.join(" ")} failed (${r.code}): ${r.stderr.trim()}`);
    }
    return r.stdout;
}

// Inherit stdio for the foreground container — this is the long-running
// `docker run -it` invocation. Returns the container's exit code so the
// wrapper can propagate it.
export async function dockerExec(args: string[]): Promise<number> {
    const cmd = new Deno.Command("docker", {
        args,
        stdin: "inherit",
        stdout: "inherit",
        stderr: "inherit",
    });
    const { code } = await cmd.output();
    return code;
}

// docker volume create — idempotent, --label arguments accepted as labels.
// Returns the volume name on success.
export async function ensureVolume(name: string, labels: readonly string[] = []): Promise<void> {
    const args = ["volume", "create"];
    for (const l of labels) args.push("--label", l);
    args.push(name);
    await dockerCapture(args);
}

// One-shot mkdir inside a throwaway container so volume-subpath mounts have
// the expected sub-directory structure on first use.
export async function ensureSubpaths(
    image: string,
    volume: string,
    subpaths: readonly string[],
): Promise<void> {
    await dockerCapture([
        "run",
        "--rm",
        "--entrypoint",
        "mkdir",
        "-v",
        `${volume}:/v`,
        image,
        "-p",
        ...subpaths.map((s) => `/v/${s}`),
    ]);
}
