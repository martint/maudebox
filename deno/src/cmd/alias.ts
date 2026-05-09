import { readAliases, writeAliases } from "../lib/config.ts";

const NAME_RE = /^[a-zA-Z_][a-zA-Z0-9_-]*$/;

export async function cmdAlias(args: string[]): Promise<number> {
    if (args[0] === "-h" || args[0] === "--help") {
        const { USAGE } = await import("../usage.ts");
        console.log(USAGE);
        return 0;
    }
    const action = args[0];
    if (!action) {
        console.error("maudebox alias: action required (add|list|rm)");
        return 1;
    }
    const rest = args.slice(1);
    const current = await readAliases();

    switch (action) {
        case "list":
            for (const [name, value] of current) console.log(`${name} = ${value}`);
            return 0;

        case "add": {
            const name = rest[0];
            const value = rest[1];
            if (!name || rest.length < 2) {
                console.error("maudebox alias add: NAME VALUE required");
                return 1;
            }
            // Catch the missing-quote mistake: `alias add cl claude --foo`
            // silently dropped the trailing args, so `cl` got recorded as
            // bare `claude`. Refuse the input and point at the fix.
            if (rest.length > 2) {
                console.error(
                    `maudebox alias add: extra arguments after VALUE — quote VALUE on the shell, e.g. 'claude --foo $MAUDEBOX_INSTANCE'`,
                );
                return 1;
            }
            if (!NAME_RE.test(name)) {
                console.error(
                    `maudebox alias add: invalid alias name '${name}' (alphanumerics, '_', and '-' only)`,
                );
                return 1;
            }
            const existed = current.has(name);
            if (existed && current.get(name) === value) {
                console.log(`Already in the user config: ${name} = ${value}`);
                return 0;
            }
            current.set(name, value!);
            await writeAliases(current);
            console.log(`${existed ? "Updated" : "Added"}: ${name} = ${value}`);
            return 0;
        }

        case "rm": {
            const name = rest[0];
            if (!name) {
                console.error("maudebox alias rm: NAME required");
                return 1;
            }
            if (!current.has(name)) {
                console.error(`Not in the user config: ${name}`);
                return 1;
            }
            current.delete(name);
            await writeAliases(current);
            console.log(`Removed: ${name}`);
            return 0;
        }

        default:
            console.error(`maudebox alias: unknown action '${action}' (expected add|list|rm)`);
            return 1;
    }
}
