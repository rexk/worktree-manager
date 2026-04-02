# jj (Jujutsu) Integration — Design Notes

## Status

Phase 1 (detection + JjCli backend) and Phase 2 (sync dual path) are implemented.
Phases 3 (workspace management) and 4 (stash replacement) are future work.

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

### Where This Matters

In colocated repos, the models overlap for branches that exist in git (they become
jj bookmarks automatically). The tension appears when:

1. **Creating new work** — git requires branch-then-worktree. jj allows
   workspace-first, name-later. wkm can't track nameless workspaces.

2. **The parent graph** — wkm stores `parent: Option<String>` where String is a
   branch name. jj tracks commit ancestry via changeset IDs. If work has no
   bookmark, wkm can't represent it in the graph.

3. **Stable identity across rebases** — jj's changeset ID survives rebases (only
   the commit hash changes). wkm currently stores neither changeset IDs nor
   tracks identity through rebases.

4. **Worktree lifecycle** — wkm ties worktree cleanup to branch deletion. In jj,
   forgetting a workspace doesn't delete commits.

### Design Options

**Option A: Stay branch-centric (current approach)**

Keep the current model. In colocated repos, branches still exist as bookmarks.
wkm remains a "branch manager" that uses jj for better operations.

- Pro: Minimal architecture change. Works today.
- Con: Can't track nameless jj workspaces.

**Option B: Add changeset ID as secondary identifier**

Extend `BranchEntry` with `changeset_id: Option<String>`. When jj is available,
store the changeset ID alongside the branch name. Operations can use whichever
is available.

- Pro: Gradual migration path. Git users unaffected.
- Con: Doesn't solve "nameless work" fully. Two identifiers to keep in sync.

**Option C: Abstract the identity layer**

Replace `BTreeMap<String, BranchEntry>` with a generalized identity:

```rust
pub enum WorkUnit {
    Branch(String),
    Change { id: String, bookmark: Option<String> },
}
```

Graph edges, WAL, errors, and operations all work with `WorkUnit` instead of
raw branch names.

- Pro: First-class jj support. Supports nameless work.
- Con: Large refactor touching graph, WAL, errors, and every operation.
  Breaks state file format (needs migration).

### Current Decision

**Option A** — stay branch-centric. In colocated repos, the jj integration
provides operational wins (cascade rebase, crash recovery, conflict handling)
without requiring a data model change. The branch-name key works because
colocated repos always have git branches that map to jj bookmarks.

If jj adoption grows and users want "name later" workflows through wkm,
Option B is the natural next step — it's additive and backward-compatible.
Option C is the end state but requires a state file migration.

## What jj Does Better (used today)

| Operation | Git approach | jj advantage |
|-----------|-------------|--------------|
| Cascade rebase | Topo-sort + temp worktrees + per-step WAL | `jj rebase -b` auto-cascades |
| Crash recovery | Custom WAL + repair | `jj op restore` atomic rollback |
| Conflict handling | Blocks sync at first conflict | Stores conflicts in commits |

## What wkm Still Provides (jj doesn't do this)

- Parent-child branch relationship tracking (jj has no concept of "branch stacks")
- Worktree lifecycle management with named storage directories
- Merge strategies (ff-only, merge-commit, squash)
- Branch stash tracking
- Swap operations (move branches between worktrees)

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
