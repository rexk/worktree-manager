---
name: wkm-merge-up
description: Merge the current branch into its parent (fast-forward by default). Use when the user says "merge up", "land this branch", or "promote to parent".
allowed-tools: Bash
---

# /wkm:merge-up — merge current branch into its parent

`wkm merge <child>` only runs when the **parent** is the currently checked-out branch. This skill handles the checkout → merge dance.

## Execution

1. Run `wkm status` (JSON) to get the current branch and its parent deterministically:

   ```
   wkm status --json
   ```

   Parse `.branch` as `child`, `.parent` as `parent`. If `.parent` is null, stop — the current branch has no parent (likely the base branch).

2. Run `wkm list --json` and confirm both `child` and `parent` are tracked. If `child` is dirty (`wkm status` showed `Working tree: dirty`), stop and ask the user to commit or stash first — wkm's merge op refuses dirty worktrees and also refuses when any descendant worktree is dirty.

3. If an op is already in progress (`In progress:` in status), stop. Tell the user to resolve it first (`wkm merge --abort`, `wkm sync --continue|--abort`).

4. Switch to the parent: `wkm checkout <parent>`. wkm handles auto-stash of the child worktree if needed.

5. Merge the child: `wkm merge <child>`. wkm picks the configured strategy (see `wkm config get merge_strategy`). If the user passed a strategy in-message ("squash merge", "merge commit"), translate to `--strategy squash` or `--strategy merge-commit`.

6. On success, run `wkm graph` to show the updated tree. Mention that the child branch still exists and was not deleted.

## Common errors wkm surfaces

- `NotAChild(<child>, <current>)`: `child`'s recorded parent is not the current branch. Either the graph was edited (`wkm set-parent`) or you checked out the wrong branch. Re-read `wkm status` and ask the user.
- `NotFastForward(<child>)`: ff strategy but parent has diverged. Suggest `/wkm:sync` first, or re-run with `--strategy merge-commit`.
- `Conflict(<child>, ...)`: non-ff merge conflict. The user must resolve in the parent's worktree, then `wkm merge --abort` or complete manually — wkm does not have a `merge --continue`.
- `DirtyWorktree(<name>)`: listed worktree has uncommitted changes; stop and report the name.
