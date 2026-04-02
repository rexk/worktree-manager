# jj (Jujutsu) Integration — Design Notes

## Strategic Positioning

**wkm is a stopgap tool for git users who have not yet adopted jj.**

After thorough analysis, we've concluded that jj (Jujutsu) natively solves the
core problems that motivated wkm's creation. Users who adopt jj should use it
directly — wkm adds little value on top of jj's native capabilities.

### What motivated wkm (from SPEC.md §1)

> Managing multiple simultaneous workstreams across AI agents and interactive
> development is painful with a single git repo directory. Git worktrees solve
> the isolation problem but have UX friction: **branch uniqueness constraints**,
> **no built-in mechanism to move branches between worktrees**, and **no
> relationship tracking between branches**.

### How jj solves each problem natively

| wkm pain point | Git limitation | jj native solution |
|----------------|---------------|-------------------|
| **Branch uniqueness constraint** | A branch can only be checked out in one worktree | No "current branch" per workspace — bookmarks are just labels, not locks |
| **Moving branches between worktrees** | Requires wkm's 5-step swap (stash, hold branch, cross-checkout, restore, WAL) | `jj edit <change>` from any workspace — no swap needed |
| **Cascade rebase** | Complex topo-sort + temp worktrees + per-step WAL (~430 lines) | `jj rebase -b` auto-cascades to all descendants |
| **Crash recovery** | Custom WAL + PID lock + repair | `jj op log` + `jj undo` / `jj op restore` — atomic operations |
| **Dirty worktree blocking** | Must stash before any worktree operation | Working copy IS a commit — no dirty state concept |
| **Conflict handling** | Blocks sync at first conflict, requires --continue/--abort | Conflicts stored in commits — can continue past them |
| **Workspace creation** | Must create branch before worktree (`git worktree add` requires branch) | `jj workspace add` — start working, name later |

### What wkm still provides that jj doesn't

- **Parent-child branch relationship tracking** — jj has commit ancestry but no
  "branch stack" concept. Tools like `jj-stack` partially fill this gap.
- **Managed storage layout** — `~/.local/share/wkm/<hash>/<id>/<repo>/` with opaque
  IDs so terminal prompts show repo names.
- **Merge strategies** — ff-only, merge-commit, squash back to parent.
- **`wkm graph`** — branch stack visualization with annotations.

This remaining value is narrow. For jj users, it amounts to a thin metadata
layer that could be a jj extension rather than a standalone tool.

### Recommendation

- **Git-only users**: wkm provides significant value. Continue using it.
- **jj users**: Use jj directly. wkm adds friction, not value.
- **Migrating from git to jj**: wkm's jj integration (Phase 1+2) can ease the
  transition, but the end state is dropping wkm in favor of native jj workflows.

---

## Implementation Status

Phase 1 (detection + JjCli backend) and Phase 2 (sync dual path) are implemented.
Further jj integration phases are **deprioritized** — the analysis above shows
that deeper integration has diminishing returns. The effort is better spent on
improving wkm for git-only users, or contributing stack-management features
upstream to the jj ecosystem.

## Architecture Overview

wkm opportunistically uses jj when the repository is **colocated** (`.jj/` + `.git/`
coexist) AND the `jj` CLI is available on PATH. Git remains the primary and default
backend. The two codepaths coexist permanently.

### Detection

`RepoContext::resolve()` checks for `.jj/` directory and runs `jj version` to set
`ctx.vcs_backend: VcsBackend::JjColocated | VcsBackend::Git`.

### Backend Dispatch

- **CLI layer**: `with_backend!` macro in `wkm-cli/src/backend.rs` constructs either
  `CliGit` or `JjCli` based on `ctx.vcs_backend`. Both implement the same 6 git traits.
- **Operations layer**: Functions like `sync()` dispatch to `sync_git()` or `sync_jj()`
  based on `ctx.vcs_backend`.

### JjCli

`JjCli` wraps `CliGit` via composition, delegating all 6 git traits by default.
Has `jj_run_ok()` / `jj_run_in()` helpers for running jj commands and
`current_op_id()` for WAL integration.

### Sync Dual Path

- `sync_git()` — Original implementation: topo-sort, temp worktrees, per-step WAL.
- `sync_jj()` — Uses `jj rebase -b <branch> -d <parent>` for native cascade rebase.
  WAL stores `jj_op_id` for rollback via `jj op restore`.

## Identity Model Tension

### The Problem

wkm's data model is **branch-name-centric**:

```
WkmState.branches: BTreeMap<String, BranchEntry>
                   ^--- branch name is the primary key

BranchEntry {
    parent: Option<String>,        // parent branch NAME
    worktree_path: Option<PathBuf>,
    ...
}
```

Branch name is the universal key for: state lookups, graph edges, WAL entries,
error messages, and every operation parameter.

jj's data model is **changeset-centric**:

```
Workspace → working-copy commit (changeset ID)
    parents: [changeset IDs]       // graph edges are changeset IDs
    bookmark: Option<name>          // branch names are optional labels
```

The changeset ID is the stable identifier. Bookmarks (branch names) are optional.
`jj workspace add` creates a workspace pointing at a commit — no bookmark required.

### Why we're not bridging this gap

Abstracting the identity layer to support both models would be a large refactor
(graph, WAL, errors, every operation) for diminishing returns. If a user has
adopted jj, they should use jj directly — not wkm with a jj backend trying to
map jj's richer model back into wkm's branch-centric one.

## What wkm's Swap Operation Does (and why jj eliminates it)

The checkout swap (`checkout.rs:155-278`) is wkm's most complex operation,
existing solely because git locks branches to worktrees:

1. Stash source worktree → WAL checkpoint
2. Stash target worktree → WAL checkpoint
3. Create `_wkm/hold/<branch>`, checkout hold to free target → WAL checkpoint
4. Cross-checkout branches between worktrees → WAL checkpoint
5. Restore stashes, delete hold branch → clear WAL

Five steps, four WAL checkpoints, a temporary branch, stash juggling.

**In jj, this entire operation is unnecessary.** Each workspace points to a
working-copy commit. Bookmarks are labels, not locks. `jj edit <change>` works
from any workspace with no swap, no stash, no hold branch. The `SwapStep` WAL
enum, the `_wkm/hold/` namespace, and the stash-during-checkout logic all exist
to work around a git limitation that jj simply doesn't have.

## File Map

| File | Role |
|------|------|
| `wkm-core/src/repo.rs` | `VcsBackend` enum + detection |
| `wkm-core/src/git/jj_cli.rs` | `JjCli` backend struct |
| `wkm-core/src/git/mod.rs` | Module declarations |
| `wkm-core/src/ops/sync/mod.rs` | Dispatcher + `sync_git()` |
| `wkm-core/src/ops/sync/jj.rs` | `sync_jj()` implementation |
| `wkm-core/src/state/types.rs` | `jj_op_id` in WAL |
| `wkm-cli/src/backend.rs` | `with_backend!` macro |
