# jj (Jujutsu) Integration — Design Notes

## Strategic Positioning

**wkm is valuable for git users AND colocated jj+git repos where jj workspaces
are currently broken.**

While jj natively solves many of wkm's core problems at the VCS layer, there is
a critical gap in jj's current workspace support that makes wkm the best option
for multi-workspace development on colocated repos.

### The Colocated Workspace Problem

In a colocated jj+git repo (`.jj/` + `.git/` coexist), neither jj nor git
provides a clean multi-workspace story today:

1. **`jj workspace add`** creates secondary workspaces with `.jj/` but **no
   `.git/`**. This means IDEs, Claude Code, GitLens, lazygit, pre-commit hooks
   — anything expecting a git repo — **break** in secondary workspaces.
   (Tracked as jj#4644, fix in progress but no firm timeline.)

2. **Raw `git worktree add`** on a colocated repo **fails** because jj always
   puts git in detached HEAD state, and `git worktree add` requires a
   `-b <branch>` flag when HEAD is detached.

3. **`wkm worktree create`** works because wkm **always creates the branch
   before the worktree**. The flow is:
   ```
   git branch feature <start-point>     # creates branch first
   git worktree add <path> feature      # succeeds — branch exists
   ```
   Secondary worktrees get a `.git` file pointing back to the main repo, so
   all git tooling works. They don't get `.jj/`, but wkm runs jj operations
   from `ctx.main_worktree` where `.jj/` exists.

| Scenario | Works? | Git tooling? | jj available? |
|---|---|---|---|
| Pure git + wkm | Yes | Yes | N/A |
| Pure jj (non-colocated) | Yes | No | Yes |
| Colocated + `jj workspace add` | Yes | **No** (no `.git/`) | Yes |
| Colocated + raw `git worktree add` | **No** (detached HEAD) | — | — |
| Colocated + wkm | **Yes** | **Yes** | In main worktree only |

**wkm is currently the only way to get multi-workspace development on colocated
repos with working git tooling.**

### What motivated wkm (from SPEC.md §1)

> Managing multiple simultaneous workstreams across AI agents and interactive
> development is painful with a single git repo directory. Git worktrees solve
> the isolation problem but have UX friction: **branch uniqueness constraints**,
> **no built-in mechanism to move branches between worktrees**, and **no
> relationship tracking between branches**.

### How jj solves each problem natively (in theory)

| wkm pain point | Git limitation | jj native solution |
|----------------|---------------|-------------------|
| **Branch uniqueness constraint** | A branch can only be checked out in one worktree | No "current branch" per workspace — bookmarks are just labels, not locks |
| **Moving branches between worktrees** | Requires wkm's 5-step swap (stash, hold branch, cross-checkout, restore, WAL) | `jj edit <change>` from any workspace — no swap needed |
| **Cascade rebase** | Complex topo-sort + temp worktrees + per-step WAL (~430 lines) | `jj rebase -b` auto-cascades to all descendants |
| **Crash recovery** | Custom WAL + PID lock + repair | `jj op log` + `jj undo` / `jj op restore` — atomic operations |
| **Dirty worktree blocking** | Must stash before any worktree operation | Working copy IS a commit — no dirty state concept |
| **Conflict handling** | Blocks sync at first conflict, requires --continue/--abort | Conflicts stored in commits — can continue past them |
| **Workspace creation** | Must create branch before worktree (`git worktree add` requires branch) | `jj workspace add` — start working, name later |

However, the colocated workspace limitation (jj#4644) means jj's multi-workspace
story is incomplete in practice. Until that's resolved, wkm provides real value
for colocated repos.

### What wkm provides beyond jj

- **Working git tooling in secondary workspaces** — the colocated workspace gap
- **Parent-child branch relationship tracking** — jj has commit ancestry but no
  "branch stack" concept. Tools like `jj-stack` partially fill this gap.
- **Managed storage layout** — `~/.local/share/wkm/<hash>/<id>/<repo>/` with opaque
  IDs so terminal prompts show repo names.
- **Merge strategies** — ff-only, merge-commit, squash back to parent.
- **`wkm graph`** — branch stack visualization with annotations.

### Recommendation

- **Git-only users**: wkm provides significant value. Continue using it.
- **Colocated jj+git users**: wkm is the best option for multi-workspace
  development until jj#4644 lands. Use wkm for workspace management, benefit
  from jj's cascade rebase via `sync_jj()`.
- **Pure jj users (non-colocated)**: Use jj directly. wkm adds friction, not value.
- **Future**: When jj#4644 lands (colocated worktree support), re-evaluate.
  At that point wkm's value for colocated repos narrows to branch stack
  tracking, storage layout, and merge strategies.

---

## Implementation Status

Phase 1 (detection + JjCli backend) and Phase 2 (sync dual path) are implemented.
Further jj integration phases are deprioritized until the colocated workspace
situation (jj#4644) is resolved and the value proposition clarifies.

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

### Why wkm worktrees work on colocated repos

wkm uses `git worktree add` (not `jj workspace add`) for creating secondary
workspaces. This is intentional:

- `git worktree add <path> <branch>` creates a worktree with a `.git` file
  pointing back to the main repo's git database. Git tooling works.
- wkm always creates the branch before calling `worktree_add`, avoiding the
  detached HEAD problem that blocks raw `git worktree add` on colocated repos.
- Secondary worktrees don't have `.jj/`, but wkm's `sync_jj()` runs jj
  commands from `ctx.main_worktree` (where `.jj/` exists), so jj operations
  still benefit from the colocated setup.
- The `JjCli::worktree_add()` delegates to `CliGit::worktree_add()` — this is
  correct and intentional. We want git worktrees, not jj workspaces.

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
(graph, WAL, errors, every operation) for diminishing returns. The colocated
workspace limitation means secondary workspaces are git-only anyway, so the
branch-centric model is the right one for wkm's worktrees.

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

However, since wkm creates git worktrees (not jj workspaces) for the colocated
compatibility reasons above, the swap operation remains necessary in wkm even
on colocated repos.

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
