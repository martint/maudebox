# ADR-0001: Record decisions in this repository

- **Status:** accepted
- **Date:** 2026-09-09

## Context

Maudebox already contains choices whose reasoning is more important than their
mechanics. Its state databases live on a Docker named volume rather than host
bind mounts; runtime sockets are separated from persistent agent state; host
configuration is mounted selectively; and project cleanup follows recorded
ownership. The code and `CLAUDE.md` describe many of these choices, but neither
provides one durable place to discover what alternatives were rejected.

This matters when a choice is revisited. A constraint whose justification has
been lost looks arbitrary, so it is likely to be removed along with the
property it protects.

`docs/SPEC.md` records what the product does. Keeping the full argument there
would make a user-facing specification argue with itself and obscure the
current contract.

## Decision

Keep numbered architectural decision records under `docs/decisions/`, one per
decision. Each record covers context, the decision, its consequences, and the
alternatives that lost.

A decision of consequence is recorded in the same change that adopts it.
Accepted records are immutable. Reversing or materially correcting one creates
a new record and marks the relationship at both ends and in the index.

## Consequences

- Contributors have a discoverable place to learn why a constraint exists.
- User-visible behavior remains readable in the product specification while
  architectural argument lives beside it.
- Changes of consequence carry an additional document and index update.
- Existing decisions are not all reconstructed immediately. Important ones can
  be backfilled when they are revisited, with the understanding that hindsight
  is less reliable than recording a decision when it is made.

## Alternatives considered

**Commit messages alone.** They are valuable history but difficult to discover
when a decision spans several changes or a contributor does not know which
commit introduced it.

**Reasoning in the product specification.** This mixes what users can rely on
with discarded designs and makes the current behavior harder to identify.

**Comments and `CLAUDE.md` alone.** They are useful close to implementation,
but are organized around files and present-day mechanics. They can disappear
when code is replaced, which is often when the original reasoning matters
most.
