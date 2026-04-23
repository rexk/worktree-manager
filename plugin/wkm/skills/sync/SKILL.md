---
name: wkm-sync
description: Run wkm sync to rebase descendants onto updated parents, and handle the --continue / --abort flow when a conflict stops the rebase. Use when the user asks to "sync", "rebase the stack", "pull parent changes", or resolves a conflict from a prior sync.
argument-hint: [--continue|--abort]
allowed-tools: Bash
---

# /wkm:sync — cascade rebase with conflict handling

`wkm sync` fetches the base branch, then topologically rebases every tracked descendant onto its parent. On conflict, wkm stops mid-rebase and expects the user to resolve in the conflicted worktree, then call `wkm sync --continue`. `wkm sync --abort` restores every branch to its pre-sync ref.

## Arguments

`$ARGUMENTS` may be empty, `--continue`, or `--abort`. Pass the flag through verbatim.

## Execution

1. Always run `wkm status` first. Note the `In progress:` line if present — that tells you whether a sync (or merge) is mid-flight.
2. Dispatch on `$ARGUMENTS`:
   - **empty**: if status shows an in-progress sync, do **not** run `wkm sync` (it will fail with `OperationInProgress`). Ask the user whether they want to continue or abort the existing sync. Otherwise run `wkm sync`.
   - **`--continue`**: run `wkm sync --continue`.
   - **`--abort`**: confirm with the user before running `wkm sync --abort` — it rewinds every synced branch and loses in-progress conflict resolution. Only skip confirmation if the user explicitly authorized aborting in the same message.
3. Parse the output:
   - `Synced: a, b, c` — report which branches advanced.
   - `Conflict in 'X'. Resolve and run \`wkm sync --continue\`.` — treat as a conflict stop.
   - `Skipped: ...` — list why (usually missing worktree or no ancestor change).
   - `All branches up to date.` — nothing to do.

## On conflict

When wkm reports a conflict:

1. Look up the conflicted branch's worktree: `wkm worktree-path <branch>`. If there is no secondary worktree, the conflict is in the main worktree.
2. In that worktree, run `git status` to show the user the conflicted paths. Do **not** auto-edit the files — the user needs to drive the resolution.
3. Explain the two resumption paths:
   - After staging the resolved files, run `/wkm:sync --continue`.
   - To give up, run `/wkm:sync --abort` (restores all pre-sync refs).
4. Do not chain `--continue` yourself unless the user explicitly says the conflicts are resolved.

## Preconditions wkm enforces

wkm refuses to start a sync if any tracked worktree is dirty; it reports `DirtyWorktree(<names>)`. If you see this, run `wkm list` to show which branches have dirty trees and let the user commit or stash them first.
