---
name: wkm-status
description: Show the current wkm branch, its parent, ahead/behind counts, and the full branch graph. Use when the user asks "where am I", "what's my stack", or "show the wkm state".
allowed-tools: Bash
---

# /wkm:status — show wkm state at a glance

Summarize the current branch's position in the wkm graph.

## Execution

Run these in sequence (both are allow-listed, so no prompt):

1. `wkm status` — prints `Branch:`, `Parent: <name> (↑N ↓M)`, optional `Remote:`, `Working tree: dirty`, and `In progress:` if an op is mid-flight.
2. `wkm graph` — prints an ASCII tree rooted at the base branch, with `wt`/`wt:<alias>`/`wt @main` markers on branches that have worktrees.

Then summarize to the user in one short paragraph:

- current branch + parent,
- ahead/behind parent,
- whether dirty,
- whether an op (sync/merge) is in progress — if so, the next step is `--continue` or `--abort`, not a new operation,
- presence of secondary worktrees from the graph.

## Failure modes

- `not initialized — run \`wkm init\` first`: offer to run `wkm init` (requires explicit user approval; `wkm init` is not allow-listed).
- Not on a branch / detached HEAD: report verbatim; do not attempt to recover automatically.
