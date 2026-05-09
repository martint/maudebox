# Sourced from /etc/bash.bashrc and ~/.bashrc by the Dockerfile. The host
# wrapper packs `name=value` lines (one per alias, terminated by newline)
# into the MAUDEBOX_ALIASES env var; this iterates over them and installs
# each as a bash alias.
#
# Values go through `printf %q`, which escapes `$` and other expansion
# triggers — so `$MAUDEBOX_INSTANCE` in an alias value stays literal in
# the definition and only expands when the alias is invoked.
if [ -n "${MAUDEBOX_ALIASES:-}" ]; then
    while IFS='=' read -r _alias_name _alias_value; do
        [ -n "$_alias_name" ] || continue
        eval "alias $_alias_name=$(printf '%q' "$_alias_value")"
    done <<< "$MAUDEBOX_ALIASES"
    unset _alias_name _alias_value
fi
