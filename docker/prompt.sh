# Bash prompt: cwd + VCS info (jj change@bookmark, or git branch).
# Modeled after the host's powerlevel10k prompt_my_jj segment, simplified.

__dev_prompt_vcs() {
    VCS_PROMPT=""

    # Prefer jj when inside a workspace. The `closest_bookmark` revset is a
    # user-defined alias on the host; embed it via --config so we don't depend
    # on a config volume mount.
    if command -v jj >/dev/null 2>&1; then
        local workspace
        if workspace=$(jj workspace root 2>/dev/null); then
            local change bookmark
            change=$(jj log --repository "$workspace" --ignore-working-copy \
                --no-graph --limit 1 --color never \
                --revisions @ -T 'self.change_id().shortest()' 2>/dev/null)
            bookmark=$(jj log --repository "$workspace" --ignore-working-copy \
                --config='revset-aliases."closest_bookmark(to)"="heads(::to & bookmarks())"' \
                --no-graph --limit 1 --color never \
                -r 'closest_bookmark(@)' \
                -T 'local_bookmarks.join(" ")' 2>/dev/null)
            VCS_PROMPT="$change"
            [ -n "$bookmark" ] && VCS_PROMPT+="@$bookmark"
            return
        fi
    fi

    # Fall back to git.
    if command -v git >/dev/null 2>&1; then
        local branch
        branch=$(git symbolic-ref --short HEAD 2>/dev/null) \
            || branch=$(git rev-parse --short HEAD 2>/dev/null)
        if [ -n "$branch" ]; then
            VCS_PROMPT="$branch"
        fi
    fi
}

PROMPT_COMMAND='__dev_prompt_vcs'

# Two-line prompt:
#   cyan/path  yellow/(vcs)
#   green/#
# \[...\] are non-printing markers so bash counts visible width correctly.
PS1='\[\033[36m\]\w\[\033[0m\]${VCS_PROMPT:+ \[\033[33m\]($VCS_PROMPT)\[\033[0m\]}\n\[\033[1;32m\]#\[\033[0m\] '
