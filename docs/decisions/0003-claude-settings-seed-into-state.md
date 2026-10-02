# ADR-0003: Seed Claude settings into writable state

- **Status:** accepted
- **Date:** 2026-10-02

## Context

Claude's host `~/.claude/settings.json` was mounted read-only at the same path
inside the container. That kept the host configuration immutable, but also
prevented Claude from persisting settings changed by commands such as
`/model`; Claude reported the failed write as `EBUSY`.

Claude settings contain both authored defaults and preferences that Claude
writes during normal use. Treating the whole file as host-owned or whole-file
state therefore gives one of those uses incorrect ownership.

## Decision

Keep the host settings file read-only, but mount it at a private bootstrap path.
When the selected Claude state namespace has no `settings.json`, the entrypoint
copies that bootstrap file into the writable `/root/.claude` state subtree.
Later launches use the namespace copy and do not overwrite it from the host.

## Consequences

- `/model` and other Claude preference changes persist within the selected
  namespace.
- The host settings file is never writable from the container.
- Existing namespaces receive the host settings file on their next launch if
  they do not already contain one.
- Changes to the host settings file affect new namespaces, but do not replace
  settings already established in an existing namespace.
- Instructions, skills, commands, and plugins remain live read-only host
  mounts.

## Alternatives considered

**Mount the host settings read-write.** This would let an agent mutate host
configuration and would make concurrent containers race over one file.

**Copy the host file on every launch.** This would erase namespace-local
preferences, including model selections, and make persistence surprising.

**Overlay the settings file.** This would preserve both views in theory, but
adds filesystem privileges and overlay semantics for a single small file.
