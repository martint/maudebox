# ADR-0002: State namespaces use separate volumes

- **Status:** accepted
- **Date:** 2026-09-09

## Context

Maudebox originally kept every agent login, session database, `gh` login,
container SSH file, and opencode XDG tree in one `maudebox-state` volume. Users
need independent identities, initially for separate Codex accounts, without
making the wrapper understand one tool's credential format. Credentials are
coupled to session databases, configuration, token refresh writes, and remote
control enrollment, so moving only a credential file would create partial and
potentially inconsistent homes.

The existing databases rely on named-volume filesystem semantics for locking,
memory mapping, and write-ahead logging. Namespace storage must preserve those
semantics, keep the legacy state usable without migration, and support safe
discovery and removal.

## Decision

A state namespace is a complete instance of the persistent state volume. The
reserved namespace `default` continues to use `maudebox-state`; every other
namespace uses its own labeled volume named `maudebox-state-<namespace>` with
the same internal subpath layout.

Namespace names are restricted to a reversible, collision-free slug format.
The launch selector is tool-independent and moves all current and future tool
state that uses this volume. Runtime-only paths such as Codex's app-server
control directory remain private to each container.

Maudebox provides `state list` and `state rm` lifecycle commands. Removal is
refused while any container uses the volume and reports that credentials,
sessions, and tool state will be forgotten. Project and ephemeral cleanup do
not own or remove namespaces.

## Consequences

- Existing users retain `maudebox-state` and do not need to log in again.
- Different namespaces have independent Claude, Codex, `gh`, SSH, and opencode
  state, while concurrent users of one namespace retain the existing database
  behavior.
- A newly bundled tool gains namespace support by placing its writable state
  in the selected volume; no public tool-specific selector is needed.
- Namespace selection is coarse-grained. Users cannot combine one namespace's
  Codex login with another namespace's `gh` or SSH state.
- Each namespace consumes a separate Docker volume and must be removed
  explicitly when no longer wanted.

## Alternatives considered

**Tool-specific selectors.** A Codex profile flag would solve the first use
case but expose the bundled tool inventory in Maudebox's public interface and
require another mechanism for every future tool.

**One selector per tool.** This permits arbitrary combinations at the cost of a
growing interface and makes it easy to split state whose credentials,
databases, sessions, and refresh behavior belong together.

**Credential-file swapping.** This requires Maudebox to understand private
tool formats and separates credentials from coupled writes and enrollment
state.

**Subdirectories in `maudebox-state`.** This would preserve named-volume
semantics, but one volume would hide namespace discovery, make usage checks and
deletion indirect, and weaken the ownership boundary presented by lifecycle
commands.
