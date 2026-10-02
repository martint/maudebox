# Decision records

Every architectural or design decision of consequence is recorded here, one
file per decision, numbered in the order it was made.

The purpose is not ceremony. The reasoning behind a decision decays faster
than the decision itself. Code can show the approach that won, but rarely the
constraint it protects, the cost accepted to obtain it, or the alternatives
that were close. Without that record, a future change has to rediscover the
argument or unknowingly removes the property the decision was protecting.

## When to write one

Write a record when a change:

- picks one approach over a viable alternative;
- constrains something hard to change later, including state layout, cleanup,
  compatibility, or the isolation boundary;
- deliberately accepts a material cost; or
- deliberately declines a capability.

Do not write one for a choice with an obvious default and no live alternative.

## Format

Name records `NNNN-short-kebab-title.md`. Each record contains:

- **Context:** the forces and constraints that made the decision necessary;
- **Decision:** the approach chosen;
- **Consequences:** benefits and costs, including the unwelcome ones; and
- **Alternatives considered:** the approaches that lost and why.

Records are immutable once accepted. A reversal gets a new record that
supersedes the old one; the old record remains and is marked superseded.

### Status

| Status | Meaning |
|---|---|
| `accepted` | True as written. |
| `amended` | The decision stands, with a correction recorded by a later ADR. |
| `superseded` | Replaced or reversed by a later ADR. |
| `proposed` | Under consideration and not yet decided. |

When a new record amends or supersedes an older one, update both records and
this index in the same change. Ordinary dependencies belong in a `Depends on:`
header rather than changing status.

## Index

| # | Decision | Status |
|---|---|---|
| [0001](0001-record-decisions.md) | Record decisions in this repository | accepted |
| [0002](0002-state-namespaces-use-separate-volumes.md) | State namespaces use separate volumes | accepted |
| [0003](0003-claude-settings-seed-into-state.md) | Seed Claude settings into writable state | accepted |
