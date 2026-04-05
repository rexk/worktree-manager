# Git Worktree Management CLI — `wkm`

## Functional Specification v0.10

---

## 1. Problem Statement

Managing multiple simultaneous workstreams across AI agents and interactive development is painful with a single git repo directory. Git worktrees solve the isolation problem but have UX friction: branch uniqueness constraints, no built-in mechanism to move branches between worktrees, and no relationship tracking between branches. Existing tools (worktrunk, git-worktree-runner, worktree-cli, Graphite, git-spice, Git Town) solve parts of this but none handle the full local-first workflow: checkout → branch off to worktree → work in parallel → sync → merge → cleanup.

> **Note on Jujutsu (jj):** The problems listed above are largely native git limitations. [Jujutsu](https://jj-vcs.dev/) — a Git-compatible VCS — solves most of them at the VCS layer: no branch-per-worktree locking, automatic cascade rebase, atomic operation log for crash recovery, and conflict storage in commits. However, jj's multi-workspace support on colocated repos is currently incomplete (jj#4644): `jj workspace add` creates secondary workspaces without `.git/`, breaking IDEs, Claude Code, and other git-dependent tooling. **wkm bridges this gap** with dual registration — it creates secondary worktrees with both `.git` (via `git worktree add`) and `.jj/` (via jj workspace creation), so both git and jj commands work in every worktree. This is the default behavior for colocated repos (see §5.7, §8.7, Appendix A).

## 2. Goals

- Define a practical, local-first workflow for managing multiple parallel workstreams with low context-switch cost.
- Provide clear branch/workspace ownership and visibility.
- Support both interactive (human + IDE) and autonomous (agent) workflows.
- Wrap git worktree mechanics with a coherent, safe UX layer.

## 3. Non-Goals

- Replacing native git conflict resolution with a custom engine.
- Building remote orchestration or PR lifecycle tooling (standard `git push` / `gh pr create` works from any worktree).
- Cross-repo orchestration (each repo is managed independently).
- Setup automation (direnv + nix flake handle per-worktree setup automatically).
- Managing devenv port allocation (per-repo configuration concern).
- Supporting the same branch checked out in multiple directories simultaneously.

## 4. Core Decisions

| Decision | Resolution | Rationale |
|----------|-----------|-----------|
| Repo model | Regular clone + linked worktrees | Bare repos have poor IDE/tooling support (VS Code [#267606], GitLens [#3090], lazygit [#2880], pre-commit [#1657]) |
| Worktree paths | Absolute only | Nix 2.33+ incompatible with git relative worktree paths due to libgit2 limitation (Nix [#14987]) |
| Primary workspace term | "Main worktree" | Aligns with git's own terminology |
| Root branch term | "Base branch" | Configurable per-repo (e.g., `main`, `master`, `develop`). Set during `wkm init`. |
| Scope | Per-repo isolation | Cross-repo orchestration is the responsibility of higher-level tools/agents |
| Implementation language | Open (Rust, Go, Zig, or shell) | Spec is language-agnostic |
| State format | Structured file in `.git/` (JSON or TOML) | Implementation decision; must include a version field for schema migration |
| Default sync strategy | Rebase | Industry standard (Graphite, git-spice, Git Town all default to rebase for child-onto-parent) |
| Default merge strategy | Fast-forward (`git merge --ff-only`) | Configurable per-repo; merge-commit and squash also supported |
| Dirty working tree | Abort operation (except checkout-in-place, which performs dirty-state swap per §8.1) | Dirty = staged changes OR unstaged changes to tracked files OR in-progress git operation (rebase, merge, cherry-pick). Untracked files do NOT count as dirty. |
| Branch freeing mechanism | Temporary branch (not detach HEAD) | When freeing a branch from a worktree, create `_wkm/hold/<branch>` instead of detaching HEAD. Preserves dirty state safely on a named branch. |
| External runtime dependency | Git CLI only | No jq, python, node, etc. Whether the implementation also uses a native git library (e.g., gitoxide) is an implementation choice. |
| Stash model | Global stash, addressed by commit hash | Git stash (`refs/stash`) is repository-global, not per-worktree. Stash commit hashes are tracked in the WAL during operations and in branch metadata after successful swaps for reliable retrieval. Stashes remain in the stash list after swap (not dropped) to protect from GC; `wkm repair` cleans up stale `wkm:`-prefixed entries not referenced by any WAL or branch metadata. |
| Worktree backend | `git` (default), `git_jj` (dual, default for colocated), `jj` (jj-only) | Colocated jj+git users expect both tools to work in secondary worktrees. Dual registration (§8.7) creates worktrees with `.git` AND `.jj/` via a temp-move technique. Validated experimentally. |

## 5. Architecture

### 5.1 Workspace Layout

```
~/upside-workspace/data-pipelines/             # Main worktree (IDE-attached)
    .git/                                       # Git database
    .git/wkm.{json,toml}                        # Worktree state file
    .git/wkm.lock                               # Lockfile (when operations in progress)

~/.local/share/wkm/<hashed-main-worktree-path>/
    <worktree-id>/
        <repo-name>/                            # Linked worktree
    <worktree-id>/
        <repo-name>/                            # Linked worktree
```

### 5.2 Directory Convention

Worktree storage path is derived from the main worktree's local filesystem path and a random worktree ID:

```
<base-dir>/<hashed-main-worktree-path>/<worktree-id>/<repo-name>/
```

The `<base-dir>` is resolved using tiered priority:

1. **Per-repo config** (`wkm.toml` → `config.storage_dir`) — fully resolved absolute path used directly (not as a base — the hash is already included).
2. **`WKM_DATA_DIR` env var** — if set, used as the base directory.
3. **`XDG_DATA_HOME` env var** — if set, `$XDG_DATA_HOME/wkm/` is used.
4. **Fallback: `~/.local/share/wkm/`** — always, on all platforms.

The fallback deliberately avoids platform-specific directories (e.g., macOS `~/Library/Application Support/`) because spaces in paths break tools like nix, devenv, and direnv.

The `<repo-name>` is the last component of the main worktree path (e.g., `/home/user/data-pipelines` → `data-pipelines`). This makes the terminal prompt show the repository name instead of an opaque ID, since terminal tools (starship, oh-my-zsh) already display the git branch.

**Worktree ID requirements:**
- `<worktree-id>` is an 8-character random lowercase hex string generated at worktree creation time.
- Not derived from the branch name — the directory is opaque and branch-agnostic. Rationale: branch names in directory paths become stale when users change branches inside worktrees (via `git checkout`, `wkm checkout` swap, or `wkm checkout -b`). Tools that fully manage worktree lifecycle (Cursor, Codex, Claude Code) use opaque IDs for this reason.
- **Collision handling:** If a generated ID already exists on disk, regenerate (statistically near-impossible with 32 bits of randomness at typical worktree counts).
- `<hashed-main-worktree-path>` uses SHA-256 (first 8 hex chars) for a deterministic, filesystem-safe repo identifier.

**This convention:**
- Is always available (no dependency on git remote being configured).
- Anchors worktree state to the local filesystem, reflecting the main ↔ worktree relationship.

### 5.3 State Storage

A structured file (JSON or TOML, implementation decision) stored at `.git/wkm.{json,toml}` in the main worktree.

**Contents:**
- **Version field** for schema migration.
- Branch parent-child relationships.
- Worktree paths for each tracked branch.
- Latest checkout-swap stash hashes for each branch (persists after successful swap until applied or cleared).
- **Global configuration overrides** (sync/merge strategy, naming convention, branch prefix).
- Creation timestamps.
- Branch descriptions (optional).
- Previous branch tracking (for checkout convenience).
- Temporary branch registry (`_wkm/*` branches with type, purpose, associated refs, stash commit hashes).
- Pending operation state (write-ahead log for crash recovery — see §8.4).
- In-progress operation indicator (like git's `MERGE_HEAD` / `REBASE_MERGE` — see §8.6).

**Write safety:**
- All state file writes must be atomic: write to a temporary file, then `rename()` to the target path. On POSIX, `rename()` is atomic and prevents partial-write corruption.
- This is critical because the WAL lives inside the state file — a corrupted state file would make crash recovery impossible.

**Access rules:**
- The state file is never committed (lives inside `.git/`).
- The CLI is the sole reader/writer.
- All commands auto-detect the main worktree (via `git worktree list`) regardless of which worktree they're run from.
- Commands do not pre-validate full state consistency. If a git operation fails due to state drift (e.g., branch deleted outside wkm), the error is surfaced with a suggestion to run `wkm repair`.

### 5.4 Temporary Branch Namespace

All tool-managed temporary branches live under the `_wkm/` prefix with sub-categories:

```
_wkm/hold/<branch>          # Swap hold: frees a branch from its worktree during checkout
_wkm/rebase/<branch>        # Rebase workspace: temporary worktree for rebasing local-only branches
```

**Rules:**
- All `_wkm/*` branches are tracked in the state file with: type, original branch, associated worktree path, stash commit hashes (if applicable), creation timestamp.
- `_wkm/*` branches are managed exclusively by the CLI. Users should not create or delete them manually.
- `wkm repair` can detect orphaned `_wkm/*` branches (tracked in state but purpose complete, or present in git but missing from state) and clean them up.
- `wkm repair` should also detect and warn if a manually created branch named `_wkm` exists, as this would conflict with the `_wkm/*` namespace (git ref naming constraint: cannot have both `_wkm` as a file and `_wkm/hold/...` as a directory under `.git/refs/heads/`).
- The `wkm:` prefix in stash messages is reserved for tool-managed stashes. `wkm repair` identifies stale stashes by this prefix. Users should avoid manually creating stashes with `wkm:` prefixed messages.

### 5.5 Branch Naming Convention

**When a name is specified:** Used as-is. If a prefix is configured, it is prepended: `<prefix>/<name>` (e.g., `rex/feature-auth`).

**When no name is specified:** Default strategy is `timestamp`: `<parent>-YYYYMMDD-HHMM` (e.g., `feature-auth-20260219-1430`). If a prefix is configured: `<prefix>/<parent>-YYYYMMDD-HHMM`.

**Configurable alternatives** (future): `random` (adjective-noun), custom strategies via config.

**Note:** `prefix` and `max_branch_length` config options apply to **branch naming** only, not worktree directory naming. Worktree directories use opaque random IDs (see §5.2).

**Contract rules:**
- No nested `/` beyond a single prefix level (avoids git ref conflicts where a path component is both a file and directory in `.git/refs/heads/`).
- Dashes (`-`) as word separators within name components.
- Timestamps in `YYYYMMDD-HHMM` format for sortability.
- Max length limit configurable to prevent unwieldy branch names.

### 5.6 Concurrency Control

Mutating commands (`checkout`, `worktree create`, `worktree remove`, `sync`, `merge`) acquire a lockfile (`.git/wkm.lock`) before modifying state.

- The lockfile contains the PID of the holding process.
- If the lockfile exists and the PID is alive: abort with a message ("another wkm operation is in progress").
- If the lockfile exists and the PID is dead: stale lock — recover automatically and proceed.
- The lockfile is removed when the operation completes or when the operation pauses for user intervention.
- **Lock release on conflict pause:** When an operation pauses for user conflict resolution (e.g., sync encounters a rebase conflict, descendant sync within merge hits a conflict), the lock is released. The WAL preserves all operation state needed to resume. `--continue` and `--abort` re-acquire the lock before proceeding.
- `git fetch` in sync runs BEFORE lock acquisition (fetch doesn't modify wkm state and can be slow).
- **Re-validate after lock acquisition:** Preconditions checked before acquiring the lock (e.g., branch existence, clean state) must be re-validated after the lock is acquired to prevent TOCTOU (time-of-check-to-time-of-use) races.
- **Internal sub-operations skip lock acquisition and in-progress checks.** When a parent operation (e.g., `merge --all`) triggers sub-operations (individual merges, descendant sync), those sub-operations execute within the parent's lock scope and do not check for in-progress operations (§8.6) — the parent's WAL is expected to be active. The lock is held for the entire parent operation duration — unless a sub-operation pauses for conflict resolution, in which case the lock is released as described above.
- Global lock for v1. Granular per-branch locking is a future optimization if contention becomes a real problem.

### 5.7 VCS Backend & Worktree Backend

wkm detects the repository's VCS backend automatically and allows per-repo configuration of how secondary worktrees are created.

**VCS Backend** (auto-detected via `RepoContext`):

| `VcsBackend` | Detection | Meaning |
|---|---|---|
| `Git` | No `.jj/` directory or `jj` not on PATH | Pure git repository |
| `JjColocated` | `.jj/` exists AND `jj version` succeeds | Colocated jj+git repository |

**Worktree Backend** (per-repo config in `wkm.toml`):

| `WorktreeBackend` | Secondary worktree has | Default for |
|---|---|---|
| `Git` | `.git` file only | Pure git repos (`VcsBackend::Git`) |
| `GitJj` | `.git` file AND `.jj/` directory (dual registration) | Colocated repos (`VcsBackend::JjColocated`) |
| `Jj` | `.jj/` directory only | Explicit opt-in only |

**Valid combinations:**

- `Git` + `Git` — pure git (default)
- `JjColocated` + `Git` — colocated, user opts out of jj for worktrees
- `JjColocated` + `GitJj` — **default for colocated** (dual registration: both tools work)
- `JjColocated` + `Jj` — colocated, jj-only worktrees
- `Git` + `GitJj` — **invalid** (error: no jj available)
- `Git` + `Jj` — **invalid** (error: no jj available)

Set via `wkm init --worktree-backend git|git-jj|jj`. Changing backend with existing worktrees is blocked ("remove existing worktrees first").

## 6. Functional Requirements

### 6.1 Workspace Model

| ID | Requirement |
|----|-------------|
| FR-1 | The system must define one canonical main worktree per repo for interactive/IDE work. |
| FR-2 | The system must support **checkout in place**: switch the current directory's branch, handling branch freeing from other worktrees internally via temporary branch creation. |
| FR-3 | The system must support **checkout to worktree**: create a new branch in a new worktree directory at the conventional location. |
| FR-4 | Checkout in place must be flexible: commits allowed after checkout, chaining checkouts allowed without mandatory return steps. |
| FR-5 | Each worktree must be self-contained: owns its own working directory, nix shell, pre-commit hooks, devenv state. |
| FR-6 | Checkout in place must preserve the full working state (staged changes, unstaged changes to tracked files, AND untracked files): captured automatically via `git stash push --include-untracked` and tracked by commit hash in the WAL. Restoration is manual — the tool prints the command (`git stash apply --index <hash>`) for the user to run when ready, preserving the staged/unstaged distinction. Git stash is repository-global — stashes are addressed by commit hash, not by worktree. Stashes remain in the stash list (`git stash list`); `wkm repair` cleans up stale entries (see §8.1). |
| FR-7 | `wkm checkout <branch>` must error if the branch does not exist. `wkm checkout -b <branch>` creates a new branch (recording current branch as parent) and switches to it. Errors if the branch already exists (same as `git checkout -b`). |
| FR-8 | `wkm checkout <branch>` where `<branch>` is the current branch is a no-op. |
| FR-9 | If `wkm worktree create <branch>` targets a branch already checked out in another worktree, error with actionable suggestions: "`wkm checkout <branch>` to move the branch to the current directory" or "`wkm wp <branch>` to navigate to the existing worktree." |

### 6.2 Branch Relationships

| ID | Requirement |
|----|-------------|
| FR-10 | The system must track explicit parent-child branch relationships as metadata in the state file. |
| FR-11 | The system must support **branch adoption**: tracking an existing branch and its parent relationship without requiring a worktree. |
| FR-12 | The system must show branch state signals: clean/dirty, ahead/behind parent, ahead/behind remote tracking branch, merge-ready/conflicted. |
| FR-13 | The system must visualize branch relationships as an ASCII graph showing worktree locations. |
| FR-43 | `wkm set-parent <new-parent> [<branch>]` changes the tracked parent of a branch and automatically runs a full sync to rebase the branch graph. The target branch must be tracked. The new parent must exist in git and be tracked (or be the base branch). Rejects cycles (new parent cannot be the branch itself or any of its descendants). After updating the parent metadata, triggers a full `sync` to rebase all branches onto their updated parents (cascade). If a rebase conflict occurs, the user resolves with `wkm sync --continue` or `wkm sync --abort`. Only the target branch's parent pointer changes; its children continue to point at the target branch (but are rebased onto its new position via cascade). |

### 6.3 Sync

| ID | Requirement |
|----|-------------|
| FR-14 | `wkm sync` must fetch from remote and rebase all child branches onto their updated parents, cascading through the branch graph. |
| FR-15 | Before cascading, sync must attempt to fast-forward the base branch to its remote tracking branch. If the base branch has diverged from remote, warn the user and continue syncing against the local state. Remote tracking branches on non-base branches are informational only — reported in `wkm status` and `wkm list` but do not influence the sync graph. |
| FR-16 | Sync must require all affected worktrees to be clean before starting. Abort with a message identifying dirty worktrees if any are found. |
| FR-17 | For branches checked out in a worktree: rebase runs inside that worktree (`git -C <path> rebase`). The branch must be the one checked out in that worktree (not a `_wkm/hold/` branch). |
| FR-18 | For branches not checked out in any worktree: create a temporary worktree (`_wkm/rebase/<branch>`), rebase there. Remove the temporary worktree if rebase is clean; keep it for conflict resolution if not. |
| FR-19 | During cascading rebase, if a conflict occurs: stop the cascade for that sub-tree. Independent branches in parallel sub-trees continue syncing. Track cascade progress in state for resumable `--continue`. |
| FR-20 | Sync does NOT perform integration. It only restacks the branch graph. |
| FR-21 | `wkm sync --continue` resumes a stopped cascade after conflict resolution. If the sync was triggered by a parent operation (e.g., `merge --all`), the parent operation resumes automatically after sync completes. |
| FR-22 | `wkm sync --abort` restores all branches to their pre-sync positions using refs saved in the WAL. If the sync has a parent `merge --all` operation, `--abort` also clears the parent WAL entry (stopping the merge sequence). Previously completed merges in the sequence are NOT rolled back. |
| FR-23 | Branches currently on a `_wkm/hold/` temp branch (freed during a checkout swap) are skipped during sync. The hold branch is transient and will be cleaned up when the swap completes or is repaired. |

### 6.4 Merge

| ID | Requirement |
|----|-------------|
| FR-24 | `wkm merge <branch>` integrates a child branch into the current branch (must be the child's parent). |
| FR-25 | Merge preconditions — abort if any are not met: (a) target branch is a child of current branch, (b) both branches are clean, (c) all descendant worktrees are clean, (d) for fast-forward strategy: child must be fast-forwardable into parent (run `wkm sync` first if not); for merge-commit/squash strategies: no divergence precondition (divergence is expected). |
| FR-26 | Confirmation prompt occurs after precondition checks and BEFORE any mutations. Skippable with `--yes`. |
| FR-27 | Default merge strategy is fast-forward (`git merge --ff-only`). Configurable per-repo: merge-commit, squash. |
| FR-28 | After successful merge: delete the child branch, remove its worktree (if any), clean up associated `_wkm/*` temp branches, remove state entries. |
| FR-29 | `wkm merge --all` merges all direct children of the current branch, sequentially. Each merge is atomic — if one fails, previously successful merges are not rolled back. If a descendant sync conflicts, `--all` stops; after the user resolves with `wkm sync --continue`, the remaining merges resume automatically (see §8.4 linked operations). Note: with fast-forward strategy, `wkm sync` should be run before `merge --all` to ensure all children are rebased onto the current parent tip — otherwise the second child will fail the FF precondition since the parent has advanced after the first merge. |
| FR-30 | If the merged child has its own children (descendants): re-parent them to the current branch in state, then internally run sync to rebase them onto the updated parent. This is automatic within the merge command. |
| FR-31 | Re-parenting must happen in state BEFORE deleting the merged branch to avoid orphaning descendants. But re-parenting happens AFTER the merge succeeds — if the merge itself fails, no state changes occur. |
| FR-32 | `wkm merge --abort` restores the pre-merge state using refs saved in the WAL: resets current branch, recreates deleted branch and worktree, restores descendant parent mappings. Only available before descendant sync begins — once the merge WAL is cleared and descendant sync starts, use `wkm sync --abort` instead. |

### 6.5 Safety

| ID | Requirement |
|----|-------------|
| FR-33 | "Dirty" is defined as: staged changes OR unstaged changes to tracked files OR an in-progress git operation (rebase, merge, cherry-pick). Untracked files do NOT count as dirty. |
| FR-34 | The system must operate fully locally without requiring push or PR creation. |
| FR-35 | The system must not depend on checking out the same branch in multiple directories. |
| FR-36 | Mutating operations must acquire a lockfile before modifying state. Concurrent operations abort with a clear message. |
| FR-37 | Any mutating `wkm` command must check for in-progress operations (sync or merge) and block if one exists, directing the user to `--continue` or `--abort` first. |

### 6.6 Automation & Scripting

| ID | Requirement |
|----|-------------|
| FR-38 | The system must support structured output (e.g., `--json` flag) for programmatic scripting and composability, in addition to human-readable output as the default. |
| FR-39 | The system must enforce consistent folder conventions for spawned workspaces. |
| FR-40 | The system must provide recovery/repair operations (see §7.3). |
| FR-41 | Cleanup prompts and confirmations must be skippable via `--yes`/`--force` flags for non-interactive/agent use. |
| FR-42 | The system must support configurable branch naming strategies (prefix, generation method, max length). |

## 7. Command Set

### 7.1 Core Operations

| Command | Purpose |
|---------|---------|
| `wkm init` | Initialize worktree tracking for the current repo. Creates state file in `.git/` and worktree storage directory. Sets the base branch (e.g., `main`). Supports `--base <branch>` to set or update the base branch. Auto-detects main worktree via `git worktree list` — can be run from any worktree. Idempotent — re-running on an initialized repo is a no-op (unless `--base` is specified to update). |
| `wkm checkout <branch>` | Switch current directory to the specified branch. If the branch is checked out in another worktree, create a `_wkm/hold/` temp branch there to free it. Captures full working state (staged + unstaged + untracked) via `git stash push --include-untracked`, tracked by commit hash. Errors if branch does not exist (use `-b` to create). No-op if already on the branch. |
| `wkm checkout -b <branch>` | Create a new branch from the current branch, record parent relationship, and switch to it in the current directory. Errors if branch already exists. Works from any worktree — parent is the branch currently checked out in that worktree. |
| `wkm adopt <branch> [-p parent]` | Adopt an existing git branch into wkm tracking. Records the parent-child relationship in state. Automatically detects if the branch is already checked out in a worktree (via `git worktree list`) and records the path. Does not create a new worktree. If `-p` is not provided, defaults to the current branch. |
| `wkm worktree create [<branch>] [-b base]` | Create a worktree at the conventional location. If `<branch>` is omitted, auto-generate a name using the configured strategy. Creates the branch from `base` (default: current branch) if it doesn't exist. `-b` sets both the creation point and the parent relationship. Records state. Prints the worktree path. Errors if branch is already checked out elsewhere. |
| `wkm worktree remove [<branch>]` | Remove the worktree for the given branch. Branch itself is kept (with parent relationship intact). Cleans up associated `_wkm/*` temp branches. Errors if run from inside the worktree being removed (user must navigate out first, e.g., `cd $(wkm wp main)`). If no branch specified, removes the worktree for the current directory's branch. |
| `wkm sync [--continue / --abort]` | Fetch remote, fast-forward base branch if possible, and restack the branch graph: cascade rebase of all child branches onto their updated parents. Does NOT integrate. Requires all affected worktrees to be clean. `--continue` resumes after conflict resolution (and resumes parent operation if linked). `--abort` restores pre-sync state (and clears parent merge --all if linked). |
| `wkm merge <branch> [--all / --yes / --abort]` | Integrate child branch into current branch (fast-forward by default). Confirmation prompt before mutations. Re-parents descendants, runs internal sync on them, cleans up merged branch/worktree/temp branches. `--all` merges all direct children sequentially (resumes automatically after descendant sync conflicts). `--yes` skips prompts. `--abort` restores pre-merge state (only before descendant sync begins). |
| `wkm set-parent <new-parent> [<branch>]` | Change the parent of a tracked branch and sync. If `<branch>` omitted, defaults to current branch. Validates that both branches are tracked (or base branch), that the new parent exists in git, and that no cycle would be created. Updates parent metadata, then runs a full sync (cascade rebase). On conflict, user resolves with `wkm sync --continue` or `--abort`. |

### 7.2 Visibility

| Command | Purpose |
|---------|---------|
| `wkm list [--json]` | Show all tracked branches with: location (main worktree / linked worktree path / local-only), parent branch, state signals (clean/dirty, ahead/behind parent, ahead/behind remote, **stashed changes pending**). `--json` for structured output. |
| `wkm graph` | ASCII branch dependency tree annotated with worktree locations and state signals. |
| `wkm status [<branch>]` | Detailed state for a branch: clean/dirty, ahead/behind parent, ahead/behind remote, merge-ready/conflicted, **stashed changes pending**. Also reports any in-progress operations (sync/merge), including the absolute path to any temporary worktree used for the operation. |
| `wkm wp <branch>` | Output the worktree path for shell navigation (alias for `worktree-path`). If the branch is in a worktree (main or linked), output that path. If the branch has no worktree, error with suggestions (`wkm worktree create <branch>` or `wkm checkout <branch>`). |

### 7.3 Maintenance

| Command | Purpose |
|---------|---------|
| `wkm config get <key>` | Get a config value. |
| `wkm config set <key> <value>` | Set a config value. Keys: `base_branch`, `merge_strategy`, `naming_strategy`, `prefix`, `max_branch_length`, `storage_dir`. |
| `wkm config list` | List all config values for the current repo. |
| `wkm repair` | Reconcile wkm state with actual filesystem and git state. Runs `git worktree repair` and `git worktree prune` to fix git-level issues. Removes stale state entries for deleted worktrees and branches that no longer exist in the git repository. Cleans up orphaned `_wkm/*` branches. Detects and warns about manually created `_wkm` branches that conflict with the namespace. Cleans up stale `wkm:`-prefixed stash entries (scans `git stash list`, drops entries whose hashes are no longer referenced by any active WAL entry **OR** branch metadata in state, and have been created more than 24 hours ago). Recovers or rolls back incomplete operations using the write-ahead log. |

### 7.4 Stash Management

| Command | Purpose |
|---------|---------|
| `wkm stash list [<branch>]` | List stashes captured by `wkm` during branch swaps. If `<branch>` specified, only show stashes for that branch. |
| `wkm stash apply [<branch>]` | Apply the most recent `wkm` stash for the specified branch (default: current branch). Runs `git stash apply --index <hash>` using the hash from branch metadata. |
| `wkm stash drop [<branch>]` | Drop the `wkm` stash reference from branch metadata. Does NOT drop the git stash commit (git GC will clean it up later if not referenced elsewhere). |

## 8. Key Mechanisms

### 8.1 Checkout in Place (Dirty-State Preservation)

**`wkm checkout feature-auth`** (from any directory in the repo):

1. If already on `feature-auth`: no-op, print message.
2. Acquire lockfile. Re-validate preconditions (branch existence, current branch state).
   - **Existence:** If the branch does not exist in git but is tracked in wkm state, error with suggestion to run `wkm repair`. If the branch does not exist at all, error with suggestion to use `wkm checkout -b`.
   - **Stashable:** Both the current worktree and worktree B (if applicable) must be in a "stashable" state. If either has an in-progress git operation (rebase, merge, cherry-pick), abort with a message identifying the conflicted worktree.
3. Determine if `feature-auth` is checked out in another worktree (worktree B).
4. **Check for stale hold branch:** If `_wkm/hold/feature-auth` already exists, check the WAL for a pending swap. If found (stale from a crash), clean it up (delete the hold branch). If not found in WAL, error and suggest `wkm repair`.
5. **Capture and clean working state** (four cases). For the purpose of this swap, a worktree "has changes" if it has staged changes, unstaged changes to tracked files, OR untracked files:
   - **Both sides have changes** (current worktree + worktree B):
     - `git stash push --include-untracked -m "wkm: <current-branch>"` in current directory. Save the stash commit hash (implementation note: prefer capturing from `git stash push` output or `git stash list` immediately after, rather than `git rev-parse stash@{0}` which is vulnerable to race conditions from external stash operations).
     - **Write partial WAL**: record `main_stash` hash immediately (so crash recovery can find it if the next step fails).
     - `git -C <worktree-B> stash push --include-untracked -m "wkm: <worktree-B-branch>"`. Save the stash commit hash.
   - **Only current worktree has changes** (worktree B clean or doesn't exist):
     - `git stash push --include-untracked -m "wkm: <current-branch>"`. Save hash.
     - **Write partial WAL**: record hash immediately.
     - `wt_stash` = empty.
   - **Only worktree B has changes** (current worktree clean):
     - `main_stash` = empty.
     - `git -C <worktree-B> stash push --include-untracked -m "wkm: <worktree-B-branch>"`. Save hash.
   - **Both clean** (no changes): Skip stashing entirely. Both stash refs = empty.
   - Note: `git stash push --include-untracked` captures staged + unstaged + untracked AND cleans the working tree in one operation. No separate clean step needed.
6. **Write full swap intent to WAL**: all stash commit hashes, source/target worktrees, original branches, current step.
7. **Free the branch:** In worktree B, `git checkout -b _wkm/hold/feature-auth` (temp branch at same commit). Record in state.
8. **Swap branches:** `git checkout feature-auth` in current directory. **Cleanup:** If the current directory was on a `_wkm/hold/*` branch before the swap, and that branch is no longer checked out in any other worktree, delete the hold branch from git and state.
9. **Clear swap state** from WAL. Release lockfile.
10. Print confirmation with both stash hashes (if any):
    ```
    Switched to feature-auth.
      Stash for feature-auth: wkm stash apply
      Stash for <previous-branch> (saved): wkm stash apply <previous-branch>
    ```
    If `wkm stash apply` fails due to conflict: "Stash `<hash>` could not be applied cleanly. Run `git stash apply <hash>` to resolve manually (without --index)."

**If `feature-auth` is NOT in another worktree:** Skip worktree B steps (freeing, stashing worktree B). Only stash current directory if it has changes.

**Stash lifecycle:** Stash commits are created via `git stash push`, tracked by hash in the WAL (during the operation) and in branch metadata (after the operation), and left in the global stash reflog after the swap completes. They are NOT dropped automatically — this ensures the stash commit objects remain protected from git GC. The user applies stashes via `wkm stash apply` or manually via `git stash apply --index <hash>`. `wkm repair` cleans up stale `wkm:`-prefixed stash entries by scanning `git stash list` and dropping entries whose hashes are no longer referenced by any active WAL entry **OR** by any branch's metadata.

**Crash recovery:** If the process crashes at any point after the partial WAL write (step 5), the WAL contains stash commit hashes and progress. `wkm repair` (or the next `wkm` command) detects the incomplete swap and either completes or rolls back based on the recorded step.

### 8.2 Checkout to Worktree

**`wkm worktree create feature-auth`**:

1. If `feature-auth` is already checked out in a worktree: error with actionable suggestions.
2. Acquire lockfile. Re-validate preconditions.
3. Generate a random 8-hex-char worktree ID. Determine the worktree path: `<storage-dir>/<worktree-id>/<repo-name>/`. If the generated directory already exists, regenerate the ID.
5. If branch `feature-auth` doesn't exist, create it from the current branch (or `-b base`).
6. If branch exists but is not tracked in wkm state: adopt it — record the parent as the `-b` value if specified, otherwise default to the **current branch**. Warn: "Branch `feature-auth` exists but is not tracked by wkm. Adopting with parent `<parent>`." (Implementation note: also check `git worktree list` in case it's checked out in a manual worktree path that differs from the convention).
7. Run `git worktree add <absolute-path> feature-auth`.
8. Record parent-child relationship and worktree path in state.
9. Release lockfile.
10. Print the worktree path.

**`wkm worktree create`** (no branch name):

1. Auto-generate a branch name using the configured strategy (default: `<current-branch>-YYYYMMDD-HHMM`, with prefix if configured).
2. Follow the same steps as above with the generated name.

**`wkm worktree remove [<branch>]`**:

1. If no branch specified: use current directory's branch.
2. If currently inside the worktree being removed: error with message — "Cannot remove the current worktree. Navigate out first: `cd $(wkm cd <main-branch>)`".
3. Acquire lockfile. Re-validate preconditions.
4. Update state: mark branch as having no worktree (keep parent-child relationship).
5. Rename `<worktree_path>` → `<worktree_path>.wkm-removing` (instant on same filesystem). If rename fails (e.g. cross-filesystem), fall back to synchronous `git worktree remove --force`.
6. Run `git worktree prune` to clean git's worktree admin entries for the now-missing path.
7. Clean up associated `_wkm/*` temp branches.
8. Release lockfile.
9. Spawn a detached `rm -rf <worktree_path>.wkm-removing` process in the background. Return immediately.
10. Print confirmation.

**Background deletion recovery:** If the background `rm -rf` is interrupted, the `.wkm-removing` directory persists harmlessly. `wkm repair` scans the storage directory for `*.wkm-removing` directories and deletes them.

### 8.3 Sync

**`wkm sync`**:

1. Run `git fetch` to update remote tracking branches (before lock — fetch doesn't modify wkm state).
2. Acquire lockfile. Verify all worktrees in the branch graph are clean. Abort with a message identifying dirty worktrees if any are found.
3. **Write sync intent to WAL**: snapshot all branch refs for abort recovery.
4. **Update base branch**: fast-forward the base branch to its remote tracking branch. First check for divergence: `git merge-base --is-ancestor <base> origin/<base>`. If not an ancestor (base has diverged from remote), warn the user and continue syncing against the local state: "Base branch `<base>` has diverged from remote. Continuing sync with local state only; your branch stack is now stale relative to remote and will need a full re-sync once divergence is resolved." If fast-forwardable: if checked out in a worktree, `git -C <worktree-path> merge --ff-only origin/<base>`; if not checked out, `git branch -f <base> origin/<base>`.
5. Walk the branch graph starting from the configured base branch. If there are no child branches, print "All branches up to date" and proceed to step 7.
   - **Skip** branches currently on a `_wkm/hold/` temp branch (transient swap state).
   - For each child branch with an outdated parent: rebase onto updated parent.
     - **Branch in a worktree:** `git -C <worktree-path> rebase <parent>`. (Precondition: the branch must be checked out in that worktree, not a hold branch.)
     - **Branch not in any worktree:** Create temporary worktree `_wkm/rebase/<branch>`, rebase there. Remove if clean.
   - Cascade: if a parent was rebased, continue to its children.
6. **Conflict handling:** If a conflict occurs during rebase:
   - Stop the cascade for that sub-tree (children of the conflicted branch are blocked).
   - Continue syncing independent parallel sub-trees.
   - Record cascade progress in state: completed branches, conflicted branch, pending branches.
   - **Release lockfile.** (WAL preserves all state needed to resume.)
   - Report the conflict location (including absolute path to the temporary worktree if used) and instructions for resolution (`wkm sync --continue` / `--abort`).
   7. If sync completes with no conflicts: clear WAL. Release lockfile.
**`wkm sync --continue`**:

1. Acquire lockfile.
2. Read incomplete sync state from WAL.
3. Verify the conflicted rebase has been resolved.
4. Resume the cascade from where it stopped.
5. Clear sync WAL.
6. If the sync was triggered by a parent operation (e.g., `merge --all`): resume the parent operation (lock remains held — the parent inherits the lock and releases it when complete).
7. If no parent operation: release lockfile.

**`wkm sync --abort`**:

1. Acquire lockfile.
2. Read pre-sync branch refs from WAL.
3. For any `_wkm/rebase/*` temporary worktree with a rebase in progress: `git -C <path> rebase --abort`.
4. Remove any `_wkm/rebase/*` temporary worktrees created during sync (`git worktree remove`).
5. Reset rebased branches to their pre-sync positions:
   - **Branch checked out in a worktree:** `git -C <worktree-path> reset --hard <saved-ref>` (cannot use `git branch -f` on a checked-out branch).
   - **Branch not checked out:** `git branch -f <branch> <saved-ref>`.
6. If the sync has a parent `merge --all` operation: clear the parent WAL entry as well (stopping the merge sequence). Previously completed merges in the sequence are NOT rolled back.
7. Clear sync WAL. Release lockfile.

### 8.4 Write-Ahead Log

Mutating operations (checkout swap, sync, merge) record their intent and progress in the state file before performing destructive steps. This enables crash recovery and `--abort`.

**Tracked state:**
- **Operation ID**: Unique identifier for the operation.
- **Parent operation ID** (nullable): Links a child operation to the operation that spawned it. Used by `merge --all` to track which merge in the sequence triggered a descendant sync, so that after `wkm sync --continue` completes, the `merge --all` sequence can resume with the next child.
- Operation type (swap, sync, merge).
- Pre-operation branch refs (commit hashes for rollback).
- Stash commit hashes (from `git stash push`).
- Source/target worktrees and branches.
- Current step in the operation.
- For sync: list of completed, conflicted, and pending branches.
- For merge --all: list of children to merge, progress (completed/pending).

**Incremental WAL writes:**
- For checkout swap: a partial WAL entry is written after each stash push (step 5 in §8.1), so that if the second stash push fails or the process crashes, the first stash hash is recoverable.
- For sync and merge: the WAL is written once before mutations begin.

**Linked operations:**
- When `merge --all` triggers a descendant sync, the merge WAL entry is cleared after the merge steps complete (branch deleted, worktree removed, state cleaned). The descendant sync writes its own WAL entry with a parent operation ID pointing back to the `merge --all` entry.
- When `wkm sync --continue` resolves the descendant conflict and completes, it reads the parent operation ID and resumes the `merge --all` sequence with the next child.
- When `wkm sync --abort` is run on a sync with a parent `merge --all`: both the sync WAL and the parent merge --all WAL are cleared. The abort stops the entire sequence. Previously completed merges are not rolled back.
- This is a two-level link only (merge → sync). No deeper nesting is supported.

**Recovery:** `wkm repair` or the next `wkm` command detects pending operations and either completes or rolls back based on recorded progress.

### 8.5 Merge

**`wkm merge feature-auth`**:

1. Acquire lockfile. Re-validate preconditions.
2. Verify preconditions:
   - Current branch is the parent of `feature-auth`.
   - Both branches are clean.
   - All descendant worktrees (if any) are clean.
   - Strategy-dependent sync check:
     - **Fast-forward**: `feature-auth` must be fast-forwardable into current branch (i.e., current branch tip is an ancestor of `feature-auth` tip). If not, error: "run `wkm sync` first."
     - **Merge-commit / squash**: No divergence precondition — divergence is expected and handled by the merge strategy.
3. **Prompt confirmation** (skippable with `--yes`).
4. **Write pre-merge snapshot to WAL**: current branch ref, child branch ref, descendant parent mappings, worktree paths.
5. **Merge:** `git merge --ff-only feature-auth` (or configured strategy: `--no-ff` for merge-commit, `--squash` for squash).
6. If merge fails: auto-rollback (clear WAL, no state was changed). Error with details.
7. **Re-parent descendants** in state: if `feature-auth` has children, update their parent to the current branch. (After merge succeeds, before deleting the merged branch.)
8. **Delete merged branch:** `git branch -d feature-auth`.
9. **Remove worktree** (if any): `git worktree remove <path>`.
10. **Clean up `_wkm/*` temp branches** associated with `feature-auth`.
11. **Remove state entries** for `feature-auth`.
12. **Clear merge WAL entry.** The merge itself is now complete.
13. **Sync descendants:** If re-parented children exist, internally run sync on them (rebase onto updated parent — should be clean since parent now contains the merged commits). This internal sync skips the cleanliness check (already verified in step 2, and lock has been held since). This sync writes its own WAL entry (with parent operation ID if part of `merge --all`). If this sync encounters a conflict, the lock is released (per §5.6) and the user resolves with `wkm sync --continue`.
14. Release lockfile (if not already released by a conflict pause in step 13).

**`wkm merge --all`**:

1. Determine the list of direct children to merge. If the list is empty: no-op, print "No children to merge."
2. **Single up-front confirmation prompt** listing all children to be merged (skippable with `--yes`). This replaces per-child prompts.
3. Acquire lockfile.
4. Write a `merge --all` WAL entry with the full list and progress tracker.
5. Execute each merge sequentially (steps 2, 4–13 of the single merge above for each child — step 1 lock acquisition, step 3 confirmation, and step 14 lock release are skipped since merge --all owns the lock lifecycle; precondition checks in step 2 still run for each child). After each child's step 12 (clear individual merge WAL), update the merge --all WAL progress to mark that child as completed before proceeding to the next child.
6. If a descendant sync conflicts: stop the sequence (lock released per §5.6). After the user resolves with `wkm sync --continue`, the remaining merges resume automatically (via the parent operation link in §8.4).
7. If using the fast-forward strategy: after each child merge, internally rebase the remaining siblings in the `merge --all` sequence onto the updated parent branch tip. This ensures they remain fast-forwardable. If a sibling rebase conflicts, follow the same conflict-handling logic as the descendant sync above.
8. If a merge itself fails (precondition not met): stop the sequence. Clear the `merge --all` WAL entry. Release lockfile. Previously completed merges are not rolled back. Print which children were merged and which remain.
9. On successful completion of all children: clear the `merge --all` WAL entry. Release lockfile.

**`wkm merge --abort`**:

1. Acquire lockfile.
2. Read pre-merge snapshot from WAL.
3. Reset current branch to pre-merge ref.
4. Recreate the child branch at its saved ref.
5. Recreate the worktree if it was removed.
6. Restore descendant parent mappings in state.
7. Clean up any `_wkm/*` branches created during the merge.
8. Clear WAL. Release lockfile.

Note: `--abort` is only available while the merge WAL entry is active (before step 12 clears it). Once the merge WAL is cleared, the merge is **permanent** — the child branch is deleted and the parent branch has advanced. If the subsequent descendant sync is aborted (`wkm sync --abort`), only the descendant rebase is undone; the merge itself is not reversed. This applies to both single merge and `merge --all`.

### 8.6 In-Progress Operation Detection

Similar to git's `MERGE_HEAD` and `REBASE_MERGE` indicators, the WAL serves as an in-progress operation indicator.

- Any mutating `wkm` command checks the WAL before starting.
- If an operation is in progress: **block** with a clear message.

```
$ wkm merge feature-api
Error: a sync operation is in progress (conflict in feature-auth).
  Run 'wkm sync --continue' after resolving conflicts.
  Run 'wkm sync --abort' to restore pre-sync state.
  Run 'wkm status' for details.
```

- `wkm status` reports in-progress operations prominently, including parent operation context (e.g., "sync in progress, triggered by merge --all — 2 of 4 children merged").
- Read-only commands (`list`, `graph`, `worktree-path`) are never blocked.

### 8.7 Dual Registration (GitJj Backend)

When `worktree_backend = "git_jj"`, secondary worktrees have both `.git` (file) and `.jj/` (directory), enabling both git and jj commands to work seamlessly — matching the main worktree experience.

**Creation sequence:**

1. `git worktree add <path> <branch>` — proper git worktree registration (`.git` file, `git worktree list` shows `[branch]`)
2. `jj workspace add <tmpdir> --name <ws-name> -r <branch>` — create jj workspace at a temp sibling path
3. Move `<tmpdir>/.jj/` into `<path>/.jj/` — the relative `repo` path in `.jj/repo` stays valid because both paths are at the same directory level
4. Write `/*` to `<path>/.jj/.gitignore` — jj creates this in main repos during `jj git init --colocate` but omits it in secondary workspaces; without it, `.jj/` appears as untracked in `git status`
5. Remove `<tmpdir>`

**Git HEAD sync protocol:**

After any jj operation that changes the working copy in a dual worktree (e.g., `jj edit`, `jj workspace update-stale`, `jj new`), git HEAD may be pointing at the wrong branch. wkm runs:

```
git symbolic-ref HEAD refs/heads/<branch>   # point HEAD at correct branch
git reset --hard <branch>                    # update index + working tree
```

Result: `git status` is clean, `git branch` shows the correct branch, `git worktree list` shows `[branch]`.

**When to sync git HEAD:** after `jj edit` (checkout), after `jj workspace update-stale` (post-sync), after `jj new` + `jj bookmark create` (checkout -b), and in `wkm repair` (reconcile drift).

**Crash recovery:** If a crash occurs between `jj edit` and git HEAD sync, `wkm repair` detects the desync (jj working copy ≠ git HEAD) and re-syncs.

## 9. Non-Functional Requirements

| ID | Requirement |
|----|-------------|
| NFR-1 | Low cognitive overhead for frequent context switching. |
| NFR-2 | Safety-first defaults: abort on dirty state, prompt before destructive actions. |
| NFR-3 | Deterministic and scriptable command behavior for automation and sub-agents. |
| NFR-4 | Idempotent operations where possible (e.g., `init` on an already-initialized repo is a no-op, `checkout` of current branch is a no-op). |
| NFR-5 | Compatibility with standard git conflict-resolution workflows. |
| NFR-6 | Only external runtime dependency is git CLI. No other tools required. |

## 10. Known Constraints

These are documented for awareness, not blockers:

| Constraint | Impact | Mitigation |
|-----------|--------|------------|
| Nix + relative worktree paths (Nix [#14987]) | Nix flake evaluation fails with relative `.git` paths | Use absolute paths exclusively (already decided) |
| devenv port conflicts | Multiple `devenv up` instances compete for ports | devenv supports `ports.<name>.allocate` for dynamic allocation; this is a per-repo config concern, not wkm's scope |
| Claude Code background tasks in worktrees ([#13087]) | Background tasks may fail to detect git repo | Agents own their worktree sessions atomically; not a practical concern |
| Git ref naming constraint | Cannot have both `feature-auth` (file) and `feature-auth/x` (directory) in `.git/refs/heads/` | Branch naming convention limits `/` to a single prefix level only; `wkm repair` detects conflicting `_wkm` branches |
| Worktree ID collisions | Two worktrees could theoretically generate the same random ID | 8-char random hex (32 bits); regenerate on collision; statistically negligible for worktree counts |
| Git stash is repository-global | `refs/stash` is shared across all worktrees; no per-worktree stash isolation | Stash commits tracked by hash in WAL and branch metadata; left in reflog to protect from GC; `wkm repair` cleans up stale entries not referenced by any active WAL or branch metadata. |
| External git operations not blocked by lockfile | Users or other tools can modify the repo while wkm holds its lock | wkm only locks its own state; git-level races are detected reactively and surfaced with `wkm repair` suggestion |
| `git worktree list` main worktree detection | The first entry in `git worktree list` is the main working tree; explicit "main" label only available since git 2.36+ | Implementation should use the first entry's path, not rely on label text |

## 11. Open Decisions (Implementation Phase)

| Decision | Options | Notes |
|----------|---------|-------|
| Implementation language | Rust, Go, Zig, shell script | Rust has strong git ecosystem (gitoxide); Go is simpler; shell is fastest to prototype |
| State file format | JSON vs TOML | Functionally equivalent at this scale |
| State storage architecture | Single file vs distributed files | Single file is simpler; distributed (per-branch, per-operation) may scale better |
| Path/branch encoding algorithm | Resolved | Main worktree paths use SHA-256 hash (first 8 hex chars); worktree directories use random 8-hex-char IDs |
| Shell integration for `wkm wp` | Shell function wrapper vs subshell | `cd $(wkm wp branch)` works but a shell function could be smoother |
| Default naming strategy | `timestamp` (default) vs `random` | Timestamp is sortable; random is more memorable |
| Operation ID generation | UUID, counter, timestamp-based | Must be unique within the state file lifetime |
| `wkm stash` subcommand | Core v1 | Branch-aware stash management (maps stash hashes to branches via state metadata). |

## 12. Acceptance Criteria

The specification is complete when:

1. The workflow model is agreed upon (single main worktree + branch off + sync + merge back). ✅
2. Same-branch multi-directory support is confirmed as non-required. ✅
3. Functional requirements are reviewed and approved as sufficient for implementation design. ✅
4. Open decisions are documented and deferred to implementation phase. ✅
5. Command set covers the full create → work → sync → merge → cleanup lifecycle. ✅

## 13. References

### Tools Researched
- [worktrunk](https://github.com/max-sixty/worktrunk) — Branch-addressed worktree management
- [git-worktree-runner (gtr)](https://github.com/coderabbitai/git-worktree-runner) — Editor/AI tool integration
- [worktree-cli](https://github.com/fnebenfuehr/worktree-cli) — AI coding assistant workflows
- [Branchlet](https://github.com/raghavpillai/branchlet) — Configurable path templates
- [agent-worktree](https://github.com/nekocode/agent-worktree) — Random name generation
- [agenttools/worktree](https://github.com/agenttools/worktree) — Issue-linked naming
- [Graphite CLI](https://graphite.dev) — Stacked branches, auto-naming from commits, prefix config
- [git-spice](https://github.com/abhinav/git-spice) — Branch metadata in git refs, prefix config
- [Git Town](https://github.com/git-town/git-town) — Branch sync and classification

### Git Documentation
- [git-worktree](https://git-scm.com/docs/git-worktree) — Official documentation
- [Nix #14987](https://github.com/nixos/nix/issues/14987) — Relative worktree incompatibility

### IDE/Tooling Compatibility
- [VS Code #267606](https://github.com/microsoft/vscode/issues/267606) — Bare repo Source Control issues
- [GitLens #3090](https://github.com/gitkraken/vscode-gitlens/issues/3090) — Bare repo crashes
- [lazygit #2880](https://github.com/jesseduffield/lazygit/issues/2880) — Bare repo path resolution
- [Claude Code #13087](https://github.com/anthropics/claude-code/issues/13087) — Background tasks in worktrees
- [devenv port allocation](https://devenv.sh/processes/) — Dynamic port allocation documentation

---

## Appendix A: Jujutsu (jj) Integration

### A.1 The Colocated Workspace Problem

In a colocated jj+git repo (`.jj/` + `.git/` coexist), neither jj nor git provides a clean multi-workspace story today:

1. **`jj workspace add`** creates secondary workspaces with `.jj/` but **no `.git/`**. This means IDEs, Claude Code, GitLens, lazygit, pre-commit hooks — anything expecting a git repo — **break** in secondary workspaces. (Tracked as jj#4644, fix in progress but no firm timeline.)

2. **Raw `git worktree add`** on a colocated repo **fails** because jj always puts git in detached HEAD state, and `git worktree add` requires a `-b <branch>` flag when HEAD is detached.

3. **`wkm worktree create`** works because wkm **always creates the branch before the worktree**:
   ```
   git branch feature <start-point>     # creates branch first
   git worktree add <path> feature      # succeeds — branch exists
   ```

| Scenario | Works? | Git tooling? | jj available? |
|---|---|---|---|
| Pure git + wkm | Yes | Yes | N/A |
| Pure jj (non-colocated) | Yes | No | Yes |
| Colocated + `jj workspace add` | Yes | **No** (no `.git/`) | Yes |
| Colocated + raw `git worktree add` | **No** (detached HEAD) | — | — |
| Colocated + wkm (`GitJj` backend) | **Yes** | **Yes** | **Yes (dual registration)** |

**wkm is the only way to get multi-workspace development on colocated repos with both git AND jj tooling working in secondary worktrees** (via dual registration, §8.7).

### A.2 How jj Solves wkm's Pain Points Natively

| wkm pain point | Git limitation | jj native solution |
|----------------|---------------|-------------------|
| **Branch uniqueness constraint** | A branch can only be checked out in one worktree | No "current branch" per workspace — bookmarks are just labels, not locks |
| **Moving branches between worktrees** | Requires wkm's 5-step swap (§8.1) | `jj edit <change>` from any workspace — no swap needed |
| **Cascade rebase** | Complex topo-sort + temp worktrees + per-step WAL (§8.3) | `jj rebase -b` auto-cascades to all descendants |
| **Crash recovery** | Custom WAL + PID lock + repair (§8.4) | `jj op log` + `jj undo` / `jj op restore` — atomic operations |
| **Dirty worktree blocking** | Must stash before any worktree operation | Working copy IS a commit — no dirty state concept |
| **Conflict handling** | Blocks sync at first conflict, requires --continue/--abort | Conflicts stored in commits — can continue past them |
| **Workspace creation** | Must create branch before worktree | `jj workspace add` — start working, name later |

However, the colocated workspace limitation (jj#4644) means jj's multi-workspace story is incomplete in practice. wkm bridges this gap with dual registration (§8.7).

### A.3 What wkm Provides Beyond jj

Even when jj resolves its workspace limitations, wkm provides value through:

- **Parent-child branch relationship tracking** — jj has commit ancestry but no "branch stack" concept
- **Managed storage layout** — `~/.local/share/wkm/<hash>/<id>/<repo>/` with opaque IDs so terminal prompts show repo names
- **Merge strategies** — ff-only, merge-commit, squash back to parent
- **`wkm graph`** — branch stack visualization with annotations
- **Concurrency control** — lockfile for wkm state mutations

### A.4 Recommendation by User Type

- **Git-only users**: wkm provides significant value. Continue using it.
- **Colocated jj+git users**: wkm is the best option for multi-workspace development. Dual registration (§8.7) gives both git and jj tooling in every worktree, plus jj's cascade rebase via `sync_jj()`.
- **Pure jj users (non-colocated)**: Use jj directly. wkm adds friction, not value.
- **Future**: When jj#4644 lands (colocated worktree support), re-evaluate. The `Jj` backend may become equivalent to `GitJj`.

### A.5 Architecture: jj Integration

wkm opportunistically uses jj when the repository is **colocated** (`.jj/` + `.git/` coexist) AND the `jj` CLI is available on PATH. Git remains the primary and default backend. The two codepaths coexist permanently.

**Detection:** `RepoContext::resolve()` checks for `.jj/` directory and runs `jj version` to set `ctx.vcs_backend: VcsBackend::JjColocated | VcsBackend::Git`.

**Backend dispatch:**

- **CLI layer**: `with_backend!` macro in `wkm-cli/src/backend.rs` constructs either `CliGit` or `JjCli` based on `ctx.vcs_backend`. Both implement the same 6 git traits.
- **Operations layer**: Functions like `sync()` dispatch to `sync_git()` or `sync_jj()` based on `ctx.vcs_backend`.

**JjCli:** Wraps `CliGit` via composition, delegating all 6 git traits by default. Has `jj_run_ok()` / `jj_run_in()` helpers for running jj commands, `current_op_id()` for WAL integration, `workspace_add()` / `workspace_forget()` / `workspace_update_stale()` for workspace management, and `sync_git_head()` for dual registration HEAD sync.

**Sync dual path:**

- `sync_git()` — Original implementation: topo-sort, temp worktrees, per-step WAL.
- `sync_jj()` — Uses `jj rebase -b <branch> -d <parent>` for native cascade rebase. For dual-registered worktrees, calls `jj workspace update-stale` + `sync_git_head()` after rebase. WAL stores `jj_op_id` for rollback via `jj op restore`.

### A.6 Identity Model

wkm's data model is **branch-name-centric**: `WkmState.branches: BTreeMap<String, BranchEntry>` where the branch name is the primary key for state lookups, graph edges, WAL entries, error messages, and every operation parameter.

jj's data model is **changeset-centric**: workspaces point to working-copy commits, graph edges are changeset IDs, and bookmarks (branch names) are optional labels.

In colocated repos, jj bookmarks and git branches are already synced — `jj git export` writes bookmarks as git branch refs, `jj git import` reads git branches as bookmarks. The same string serves as wkm state key, jj bookmark name, and git branch name. No separate mapping needed.

### A.7 Swap Operation Simplification

The checkout swap (§8.1) is wkm's most complex operation, existing solely because git locks branches to worktrees: 5 steps, 4 WAL checkpoints, a temporary branch, stash juggling.

**In jj, this is unnecessary.** Each workspace points to a working-copy commit. Bookmarks are labels, not locks. `jj edit <change>` works from any workspace with no swap, no stash, no hold branch.

With the `GitJj` backend (dual registration), `jj edit <bookmark>` + git HEAD sync (§8.7) replaces the 5-step swap. The `SwapStep` WAL enum, the `_wkm/hold/` namespace, and the stash-during-checkout logic are only needed in pure `Git` backend mode.

### A.8 Implementation Status

| Phase | Description | Status |
|-------|-------------|--------|
| **Phase 1** | VCS detection + `JjCli` backend | Implemented |
| **Phase 2** | Sync dual path (`sync_jj` with `jj rebase -b`) | Implemented |
| **Phase 3** | Dual registration (`WorktreeBackend::GitJj`) | Implemented |

**Validated fixes (Phase 2):** Working tree desync after `jj rebase` + `jj git export` (fixed with `git reset --hard` / `sync_git_head`). Missing dirty worktree check in `sync_jj` (fixed by adding dirty check matching `sync_git`). Both covered by integration tests.

### A.9 jj Integration File Map

| File | Role |
|------|------|
| `wkm-core/src/repo.rs` | `VcsBackend` enum + detection |
| `wkm-core/src/git/jj_cli.rs` | `JjCli` backend, `sync_git_head()`, workspace helpers |
| `wkm-core/src/git/mod.rs` | Module declarations |
| `wkm-core/src/ops/sync/mod.rs` | Dispatcher + `sync_git()` |
| `wkm-core/src/ops/sync/jj.rs` | `sync_jj()` + dual registration tests |
| `wkm-core/src/ops/worktree.rs` | `create()` with dual registration, `setup_jj_workspace()` |
| `wkm-core/src/ops/init.rs` | `--worktree-backend` option, auto-default for colocated |
| `wkm-core/src/state/types.rs` | `WorktreeBackend` enum, `jj_workspace_name`, `jj_op_id` in WAL |
| `wkm-cli/src/backend.rs` | `with_backend!` macro |
| `wkm-cli/src/commands/init.rs` | CLI `--worktree-backend` flag |

