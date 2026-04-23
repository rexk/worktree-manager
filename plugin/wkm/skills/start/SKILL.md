---
name: wkm-start
description: Start a new wkm-tracked branch as a child of the current branch, optionally in a secondary worktree. Use when the user asks to "start", "branch off", "stack a branch", or "create a worktree".
argument-hint: <branch-name> [--worktree]
allowed-tools: Bash
---

# /wkm:start — branch off the current branch

Create a new branch tracked by wkm, parented at the branch currently checked out in the cwd.

## Arguments

`$ARGUMENTS` is the branch name, optionally followed by `--worktree` to create a secondary worktree under the configured storage directory instead of reusing the current worktree.

Parse defensively:

- If `$ARGUMENTS` is empty, ask the user for the branch name rather than inventing one.
- Split on whitespace. The first token is the branch name; treat `--worktree` / `-w` as the only recognized flag.
- If the branch name has a space or invalid character, surface the error from wkm rather than silently rewriting it.

## Execution

1. Run `wkm status` first so you know what the parent will be (the current branch). Mention it to the user in the response.
2. Run `wkm config get prefix` and `wkm config get max_branch_length`. If either is set, verify `<branch-name>` matches; if it does not, tell the user what the config expects before running any mutating command. Do **not** silently rewrite their name.
3. Run the creation command:
   - No `--worktree`: `wkm checkout -b <branch-name>` — creates the branch and switches the current worktree to it.
   - With `--worktree`: `wkm worktree create <branch-name>` — creates a new branch **and** a secondary worktree under the storage directory. Report the printed `Worktree: <path>` line back to the user so they know where to `cd`.
4. After success, run `wkm status` again so the user sees the new branch is current (no-worktree flow) or so you can remind them the current worktree did not change (worktree flow).

## When to refuse / escalate

- If `wkm status` shows `In progress: <op>`, stop. Tell the user to finish or abort the in-progress op (`wkm sync --continue|--abort`, `wkm merge --abort`) before starting a new branch.
- If the current worktree is dirty and the user asked for the no-worktree form, `wkm checkout -b` will auto-stash; that is fine, but mention it so the user is not surprised later.
- If wkm is not initialized (`Not initialized` error), stop and ask whether to run `wkm init` first.

## Examples

- `/wkm:start feat-login` → `wkm checkout -b feat-login`
- `/wkm:start feat-login --worktree` → `wkm worktree create feat-login`
