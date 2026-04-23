---
name: wkm-repair
description: Reconcile wkm state against the real git worktree/branch state, clear stale locks, and prune missing worktrees. Use when wkm complains about inconsistent state, stale locks, or after a crash.
allowed-tools: Bash
---

# /wkm:repair — reconcile wkm state

`wkm repair` is the recovery hammer. It:

- removes stale PID-based lockfiles,
- runs `git worktree repair` and `prune`,
- clears an incomplete WAL entry from a crashed operation,
- removes state entries whose branches no longer exist,
- prunes entries whose secondary worktree path is missing,
- clears `worktree_path` when the recorded path no longer exists,
- updates `worktree_path` when git moved the worktree,
- auto-adopts untracked branches living in wkm-managed worktrees,
- deletes orphaned `_wkm/*` helper branches,
- cleans up pending-removal entries.

## Execution

1. Before running, tell the user what `wkm repair` does at a high level — this is a mutating recovery op and they should not run it blindly. Ask for confirmation unless they already said "run repair" or equivalent.
2. Run `wkm repair`. Every action prints one line; if nothing was wrong, it prints `Nothing to repair.`.
3. Echo the printed actions to the user and then run `wkm graph` to show the reconciled tree.

## When repair is the right answer

- Error: `lock held by PID <n>` but that PID is dead — repair clears the stale lock.
- Error: `OperationInProgress` after a crash, when `wkm sync --continue` and `wkm sync --abort` both fail because the WAL is unrecoverable.
- `wkm list` shows a branch with a worktree path that no longer exists on disk.
- You manually `rm -rf`'d a secondary worktree directory.

## When repair is **not** the right answer

- A live conflict from `wkm sync` — use `wkm sync --continue` or `--abort`.
- A normal dirty-worktree rejection — commit or stash first.
- Wanting to drop a branch from tracking — use `wkm drop <branch>` (repair will not remove a tracked branch whose worktree is merely clean).
