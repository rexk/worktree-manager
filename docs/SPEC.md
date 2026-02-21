# Git Worktree Management CLI — `uwt`

## Functional Specification v0.10

---

## 1. Problem Statement

Managing multiple simultaneous workstreams across AI agents and interactive development is painful with a single git repo directory. Git worktrees solve the isolation problem but have UX friction: branch uniqueness constraints, no built-in mechanism to move branches between worktrees, and no relationship tracking between branches. Existing tools (worktrunk, git-worktree-runner, worktree-cli, Graphite, git-spice, Git Town) solve parts of this but none handle the full local-first workflow: checkout → branch off to worktree → work in parallel → sync → merge → cleanup.

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
| Root branch term | "Base branch" | Configurable per-repo (e.g., `main`, `master`, `develop`). Set during `uwt init`. |
| Scope | Per-repo isolation | Cross-repo orchestration is the responsibility of higher-level tools/agents |
| Implementation language | Open (Rust, Go, Zig, or shell) | Spec is language-agnostic |
| State format | Structured file in `.git/` (JSON or TOML) | Implementation decision; must include a version field for schema migration |
| Default sync strategy | Rebase | Industry standard (Graphite, git-spice, Git Town all default to rebase for child-onto-parent) |
| Default merge strategy | Fast-forward (`git merge --ff-only`) | Configurable per-repo; merge-commit and squash also supported |
| Dirty working tree | Abort operation (except checkout-in-place, which performs dirty-state swap per §8.1) | Dirty = staged changes OR unstaged changes to tracked files OR in-progress git operation (rebase, merge, cherry-pick). Untracked files do NOT count as dirty. |
| Branch freeing mechanism | Temporary branch (not detach HEAD) | When freeing a branch from a worktree, create `_uwt/hold/<branch>` instead of detaching HEAD. Preserves dirty state safely on a named branch. |
| External runtime dependency | Git CLI only | No jq, python, node, etc. Whether the implementation also uses a native git library (e.g., gitoxide) is an implementation choice. |
| Stash model | Global stash, addressed by commit hash | Git stash (`refs/stash`) is repository-global, not per-worktree. Stash commit hashes are tracked in the WAL for reliable retrieval from any worktree. Stashes remain in the stash list after swap (not dropped) to protect from GC; `uwt repair` cleans up stale `uwt:`-prefixed entries. |

## 5. Architecture

### 5.1 Workspace Layout

```
~/upside-workspace/data-pipelines/             # Main worktree (IDE-attached)
    .git/                                       # Git database
    .git/uwt.{json,toml}                        # Worktree state file
    .git/uwt.lock                               # Lockfile (when operations in progress)

~/.local/share/uwt/<encoded-main-worktree-path>/
    <encoded-branch-name>/                      # Linked worktree
    <encoded-branch-name>/                      # Linked worktree
```

### 5.2 Directory Convention

Worktree storage path is derived from the main worktree's local filesystem path and branch name, both encoded to produce unique directory names:

```
~/.local/share/uwt/<encoded-main-worktree-path>/<encoded-branch-name>/
```

**Encoding requirements:**
- The encoding must be deterministic (same input → same encoded directory).
- The encoding is **best-effort collision-resistant** — designed to avoid collisions but not guaranteed collision-free.
- Both the main worktree path and the branch name are encoded using the same algorithm.
- Branch names containing `/` (e.g., `rex/feature-auth`) must be encoded to a flat directory name (not nested directories).
- **Collision handling:** If a target directory already exists for a different branch, abort with a clear error suggesting the user provide an explicit name. No silent overwriting.
- The exact encoding algorithm is an implementation decision.

**This convention:**
- Is always available (no dependency on git remote being configured).
- Anchors worktree state to the local filesystem, reflecting the main ↔ worktree relationship.

### 5.3 State Storage

A structured file (JSON or TOML, implementation decision) stored at `.git/uwt.{json,toml}` in the main worktree.

**Contents:**
- **Version field** for schema migration.
- Branch parent-child relationships.
- Worktree paths for each tracked branch.
- Creation timestamps.
- Branch descriptions (optional).
- Previous branch tracking (for checkout convenience).
- Temporary branch registry (`_uwt/*` branches with type, purpose, associated refs, stash commit hashes).
- Pending operation state (write-ahead log for crash recovery — see §8.4).
- In-progress operation indicator (like git's `MERGE_HEAD` / `REBASE_MERGE` — see §8.6).

**Write safety:**
- All state file writes must be atomic: write to a temporary file, then `rename()` to the target path. On POSIX, `rename()` is atomic and prevents partial-write corruption.
- This is critical because the WAL lives inside the state file — a corrupted state file would make crash recovery impossible.

**Access rules:**
- The state file is never committed (lives inside `.git/`).
- The CLI is the sole reader/writer.
- All commands auto-detect the main worktree (via `git worktree list`) regardless of which worktree they're run from.
- Commands do not pre-validate full state consistency. If a git operation fails due to state drift (e.g., branch deleted outside uwt), the error is surfaced with a suggestion to run `uwt repair`.

### 5.4 Temporary Branch Namespace

All tool-managed temporary branches live under the `_uwt/` prefix with sub-categories:

```
_uwt/hold/<branch>          # Swap hold: frees a branch from its worktree during checkout
_uwt/rebase/<branch>        # Rebase workspace: temporary worktree for rebasing local-only branches
```

**Rules:**
- All `_uwt/*` branches are tracked in the state file with: type, original branch, associated worktree path, stash commit hashes (if applicable), creation timestamp.
- `_uwt/*` branches are managed exclusively by the CLI. Users should not create or delete them manually.
- `uwt repair` can detect orphaned `_uwt/*` branches (tracked in state but purpose complete, or present in git but missing from state) and clean them up.
- `uwt repair` should also detect and warn if a manually created branch named `_uwt` exists, as this would conflict with the `_uwt/*` namespace (git ref naming constraint: cannot have both `_uwt` as a file and `_uwt/hold/...` as a directory under `.git/refs/heads/`).
- The `uwt:` prefix in stash messages is reserved for tool-managed stashes. `uwt repair` identifies stale stashes by this prefix. Users should avoid manually creating stashes with `uwt:` prefixed messages.

### 5.5 Branch Naming Convention

**When a name is specified:** Used as-is. If a prefix is configured, it is prepended: `<prefix>/<name>` (e.g., `rex/feature-auth`).

**When no name is specified:** Default strategy is `timestamp`: `<parent>-YYYYMMDD-HHMM` (e.g., `feature-auth-20260219-1430`). If a prefix is configured: `<prefix>/<parent>-YYYYMMDD-HHMM`.

**Configurable alternatives** (future): `random` (adjective-noun), custom strategies via config.

**Contract rules:**
- No nested `/` beyond a single prefix level (avoids git ref conflicts where a path component is both a file and directory in `.git/refs/heads/`).
- Dashes (`-`) as word separators within name components.
- Timestamps in `YYYYMMDD-HHMM` format for sortability.
- Max length limit configurable to prevent unwieldy branch names.

### 5.6 Concurrency Control

Mutating commands (`checkout`, `worktree create`, `worktree remove`, `sync`, `merge`) acquire a lockfile (`.git/uwt.lock`) before modifying state.

- The lockfile contains the PID of the holding process.
- If the lockfile exists and the PID is alive: abort with a message ("another uwt operation is in progress").
- If the lockfile exists and the PID is dead: stale lock — recover automatically and proceed.
- The lockfile is removed when the operation completes or when the operation pauses for user intervention.
- **Lock release on conflict pause:** When an operation pauses for user conflict resolution (e.g., sync encounters a rebase conflict, grandchild sync within merge hits a conflict), the lock is released. The WAL preserves all operation state needed to resume. `--continue` and `--abort` re-acquire the lock before proceeding.
- `git fetch` in sync runs BEFORE lock acquisition (fetch doesn't modify uwt state and can be slow).
- **Re-validate after lock acquisition:** Preconditions checked before acquiring the lock (e.g., branch existence, clean state) must be re-validated after the lock is acquired to prevent TOCTOU (time-of-check-to-time-of-use) races.
- **Internal sub-operations skip lock acquisition and in-progress checks.** When a parent operation (e.g., `merge --all`) triggers sub-operations (individual merges, grandchild sync), those sub-operations execute within the parent's lock scope and do not check for in-progress operations (§8.6) — the parent's WAL is expected to be active. The lock is held for the entire parent operation duration — unless a sub-operation pauses for conflict resolution, in which case the lock is released as described above.
- Global lock for v1. Granular per-branch locking is a future optimization if contention becomes a real problem.

## 6. Functional Requirements

### 6.1 Workspace Model

| ID | Requirement |
|----|-------------|
| FR-1 | The system must define one canonical main worktree per repo for interactive/IDE work. |
| FR-2 | The system must support **checkout in place**: switch the current directory's branch, handling branch freeing from other worktrees internally via temporary branch creation. |
| FR-3 | The system must support **checkout to worktree**: create a new branch in a new worktree directory at the conventional location. |
| FR-4 | Checkout in place must be flexible: commits allowed after checkout, chaining checkouts allowed without mandatory return steps. |
| FR-5 | Each worktree must be self-contained: owns its own working directory, nix shell, pre-commit hooks, devenv state. |
| FR-6 | Checkout in place must preserve the full working state (staged changes, unstaged changes to tracked files, AND untracked files): captured automatically via `git stash push --include-untracked` and tracked by commit hash in the WAL. Restoration is manual — the tool prints the command (`git stash apply --index <hash>`) for the user to run when ready, preserving the staged/unstaged distinction. Git stash is repository-global — stashes are addressed by commit hash, not by worktree. Stashes remain in the stash list (`git stash list`); `uwt repair` cleans up stale entries (see §8.1). |
| FR-7 | `uwt checkout <branch>` must error if the branch does not exist. `uwt checkout -b <branch>` creates a new branch (recording current branch as parent) and switches to it. Errors if the branch already exists (same as `git checkout -b`). |
| FR-8 | `uwt checkout <branch>` where `<branch>` is the current branch is a no-op. |
| FR-9 | If `uwt worktree create <branch>` targets a branch already checked out in another worktree, error with actionable suggestions: "`uwt checkout <branch>` to move the branch to the current directory" or "`uwt cd <branch>` to navigate to the existing worktree." |

### 6.2 Branch Relationships

| ID | Requirement |
|----|-------------|
| FR-10 | The system must track explicit parent-child branch relationships as metadata in the state file. |
| FR-11 | The system must show branch state signals: clean/dirty, ahead/behind parent, ahead/behind remote tracking branch, merge-ready/conflicted. |
| FR-12 | The system must visualize branch relationships as an ASCII graph showing worktree locations. |

### 6.3 Sync

| ID | Requirement |
|----|-------------|
| FR-13 | `uwt sync` must fetch from remote and rebase all child branches onto their updated parents, cascading through the branch graph. |
| FR-14 | Before cascading, sync must attempt to fast-forward the base branch to its remote tracking branch. If the base branch has diverged from remote, warn the user and continue syncing against the local state. Remote tracking branches on non-base branches are informational only — reported in `uwt status` and `uwt list` but do not influence the sync graph. |
| FR-15 | Sync must require all affected worktrees to be clean before starting. Abort with a message identifying dirty worktrees if any are found. |
| FR-16 | For branches checked out in a worktree: rebase runs inside that worktree (`git -C <path> rebase`). The branch must be the one checked out in that worktree (not a `_uwt/hold/` branch). |
| FR-17 | For branches not checked out in any worktree: create a temporary worktree (`_uwt/rebase/<branch>`), rebase there. Remove the temporary worktree if rebase is clean; keep it for conflict resolution if not. |
| FR-18 | During cascading rebase, if a conflict occurs: stop the cascade for that sub-tree. Independent branches in parallel sub-trees continue syncing. Track cascade progress in state for resumable `--continue`. |
| FR-19 | Sync does NOT perform integration. It only restacks the branch graph. |
| FR-20 | `uwt sync --continue` resumes a stopped cascade after conflict resolution. If the sync was triggered by a parent operation (e.g., `merge --all`), the parent operation resumes automatically after sync completes. |
| FR-21 | `uwt sync --abort` restores all branches to their pre-sync positions using refs saved in the WAL. If the sync has a parent `merge --all` operation, `--abort` also clears the parent WAL entry (stopping the merge sequence). Previously completed merges in the sequence are NOT rolled back. |
| FR-22 | Branches currently on a `_uwt/hold/` temp branch (freed during a checkout swap) are skipped during sync. The hold branch is transient and will be cleaned up when the swap completes or is repaired. |

### 6.4 Merge

| ID | Requirement |
|----|-------------|
| FR-23 | `uwt merge <branch>` integrates a child branch into the current branch (must be the child's parent). |
| FR-24 | Merge preconditions — abort if any are not met: (a) target branch is a child of current branch, (b) both branches are clean, (c) all grandchild worktrees are clean, (d) for fast-forward strategy: child must be fast-forwardable into parent (run `uwt sync` first if not); for merge-commit/squash strategies: no divergence precondition (divergence is expected). |
| FR-25 | Confirmation prompt occurs after precondition checks and BEFORE any mutations. Skippable with `--yes`. |
| FR-26 | Default merge strategy is fast-forward (`git merge --ff-only`). Configurable per-repo: merge-commit, squash. |
| FR-27 | After successful merge: delete the child branch, remove its worktree (if any), clean up associated `_uwt/*` temp branches, remove state entries. |
| FR-28 | `uwt merge --all` merges all direct children of the current branch, sequentially. Each merge is atomic — if one fails, previously successful merges are not rolled back. If a grandchild sync conflicts, `--all` stops; after the user resolves with `uwt sync --continue`, the remaining merges resume automatically (see §8.4 linked operations). Note: with fast-forward strategy, `uwt sync` should be run before `merge --all` to ensure all children are rebased onto the current parent tip — otherwise the second child will fail the FF precondition since the parent has advanced after the first merge. |
| FR-29 | If the merged child has its own children (grandchildren): re-parent them to the current branch in state, then internally run sync to rebase them onto the updated parent. This is automatic within the merge command. |
| FR-30 | Re-parenting must happen in state BEFORE deleting the merged branch to avoid orphaning grandchildren. But re-parenting happens AFTER the merge succeeds — if the merge itself fails, no state changes occur. |
| FR-31 | `uwt merge --abort` restores the pre-merge state using refs saved in the WAL: resets current branch, recreates deleted branch and worktree, restores grandchild parent mappings. Only available before grandchild sync begins — once the merge WAL is cleared and grandchild sync starts, use `uwt sync --abort` instead. |

### 6.5 Safety

| ID | Requirement |
|----|-------------|
| FR-32 | "Dirty" is defined as: staged changes OR unstaged changes to tracked files OR an in-progress git operation (rebase, merge, cherry-pick). Untracked files do NOT count as dirty. |
| FR-33 | The system must operate fully locally without requiring push or PR creation. |
| FR-34 | The system must not depend on checking out the same branch in multiple directories. |
| FR-35 | Mutating operations must acquire a lockfile before modifying state. Concurrent operations abort with a clear message. |
| FR-36 | Any mutating `uwt` command must check for in-progress operations (sync or merge) and block if one exists, directing the user to `--continue` or `--abort` first. |

### 6.6 Automation & Scripting

| ID | Requirement |
|----|-------------|
| FR-37 | The system must support structured output (e.g., `--json` flag) for programmatic scripting and composability, in addition to human-readable output as the default. |
| FR-38 | The system must enforce deterministic folder conventions for spawned workspaces. |
| FR-39 | The system must provide recovery/repair operations (see §7.3). |
| FR-40 | Cleanup prompts and confirmations must be skippable via `--yes`/`--force` flags for non-interactive/agent use. |
| FR-41 | The system must support configurable branch naming strategies (prefix, generation method, max length). |

## 7. Command Set

### 7.1 Core Operations

| Command | Purpose |
|---------|---------|
| `uwt init` | Initialize worktree tracking for the current repo. Creates state file in `.git/` and worktree storage directory. Sets the base branch (e.g., `main`). Supports `--base <branch>` to set or update the base branch. Auto-detects main worktree via `git worktree list` — can be run from any worktree. Idempotent — re-running on an initialized repo is a no-op (unless `--base` is specified to update). |
| `uwt checkout <branch>` | Switch current directory to the specified branch. If the branch is checked out in another worktree, create a `_uwt/hold/` temp branch there to free it. Captures full working state (staged + unstaged + untracked) via `git stash push --include-untracked`, tracked by commit hash. Errors if branch does not exist (use `-b` to create). No-op if already on the branch. |
| `uwt checkout -b <branch>` | Create a new branch from the current branch, record parent relationship, and switch to it in the current directory. Errors if branch already exists. Works from any worktree — parent is the branch currently checked out in that worktree. |
| `uwt worktree create [<branch>] [-b base]` | Create a worktree at the conventional location. If `<branch>` is omitted, auto-generate a name using the configured strategy. Creates the branch from `base` (default: current branch) if it doesn't exist. `-b` sets both the creation point and the parent relationship. Records state. Prints the worktree path. Errors if branch is already checked out elsewhere. |
| `uwt worktree remove [<branch>]` | Remove the worktree for the given branch. Branch itself is kept (with parent relationship intact). Cleans up associated `_uwt/*` temp branches. Errors if run from inside the worktree being removed (user must navigate out first, e.g., `cd $(uwt cd main)`). If no branch specified, removes the worktree for the current directory's branch. |
| `uwt sync [--continue / --abort]` | Fetch remote, fast-forward base branch if possible, and restack the branch graph: cascade rebase of all child branches onto their updated parents. Does NOT integrate. Requires all affected worktrees to be clean. `--continue` resumes after conflict resolution (and resumes parent operation if linked). `--abort` restores pre-sync state (and clears parent merge --all if linked). |
| `uwt merge <branch> [--all / --yes / --abort]` | Integrate child branch into current branch (fast-forward by default). Confirmation prompt before mutations. Re-parents grandchildren, runs internal sync on them, cleans up merged branch/worktree/temp branches. `--all` merges all direct children sequentially (resumes automatically after grandchild sync conflicts). `--yes` skips prompts. `--abort` restores pre-merge state (only before grandchild sync begins). |

### 7.2 Visibility

| Command | Purpose |
|---------|---------|
| `uwt list [--json]` | Show all tracked branches with: location (main worktree / linked worktree path / local-only), parent branch, state signals (clean/dirty, ahead/behind parent, ahead/behind remote). `--json` for structured output. |
| `uwt graph` | ASCII branch dependency tree annotated with worktree locations and state signals. |
| `uwt status [<branch>]` | Detailed state for a branch: clean/dirty, ahead/behind parent, ahead/behind remote, merge-ready/conflicted. Also reports any in-progress operations (sync/merge) and remote tracking divergence ("force-push required"). |
| `uwt cd <branch>` | Output the worktree path for shell navigation. If the branch is in a worktree (main or linked), output that path. If the branch has no worktree, error with suggestions (`uwt worktree create <branch>` or `uwt checkout <branch>`). |

### 7.3 Maintenance

| Command | Purpose |
|---------|---------|
| `uwt repair` | Reconcile uwt state with actual filesystem and git state. Runs `git worktree repair` and `git worktree prune` to fix git-level issues. Removes stale state entries for deleted worktrees. Cleans up orphaned `_uwt/*` branches. Detects and warns about manually created `_uwt` branches that conflict with the namespace. Cleans up stale `uwt:`-prefixed stash entries (scans `git stash list`, drops entries whose hashes are no longer referenced by any active WAL entry). Recovers or rolls back incomplete operations using the write-ahead log. |

## 8. Key Mechanisms

### 8.1 Checkout in Place (Dirty-State Preservation)

**`uwt checkout feature-auth`** (from any directory in the repo):

1. If already on `feature-auth`: no-op, print message.
2. Acquire lockfile. Re-validate preconditions (branch existence, current branch state). If the branch does not exist in git but is tracked in uwt state, error with suggestion to run `uwt repair`. If the branch does not exist at all, error with suggestion to use `uwt checkout -b`.
3. Determine if `feature-auth` is checked out in another worktree (worktree B).
4. **Check for stale hold branch:** If `_uwt/hold/feature-auth` already exists, check the WAL for a pending swap. If found (stale from a crash), clean it up (delete the hold branch). If not found in WAL, error and suggest `uwt repair`.
5. **Capture and clean working state** (four cases):
   - **Both sides dirty** (current worktree + worktree B):
     - `git stash push --include-untracked -m "uwt: <current-branch>"` in current directory. Save the stash commit hash (implementation note: prefer capturing from `git stash push` output or `git stash list` immediately after, rather than `git rev-parse stash@{0}` which is vulnerable to race conditions from external stash operations).
     - **Write partial WAL**: record `main_stash` hash immediately (so crash recovery can find it if the next step fails).
     - `git -C <worktree-B> stash push --include-untracked -m "uwt: <worktree-B-branch>"`. Save the stash commit hash.
   - **Only current worktree dirty** (worktree B clean or doesn't exist):
     - `git stash push --include-untracked -m "uwt: <current-branch>"`. Save hash.
     - **Write partial WAL**: record hash immediately.
     - `wt_stash` = empty.
   - **Only worktree B dirty** (current worktree clean):
     - `main_stash` = empty.
     - `git -C <worktree-B> stash push --include-untracked -m "uwt: <worktree-B-branch>"`. Save hash.
   - **Both clean**: Skip stashing entirely. Both stash refs = empty.
   - Note: `git stash push --include-untracked` captures staged + unstaged + untracked AND cleans the working tree in one operation. No separate clean step needed.
6. **Write full swap intent to WAL**: all stash commit hashes, source/target worktrees, original branches, current step.
7. **Free the branch:** In worktree B, `git checkout -b _uwt/hold/feature-auth` (temp branch at same commit). Record in state.
8. **Swap branches:** `git checkout feature-auth` in current directory.
9. **Clear swap state** from WAL. Release lockfile.
10. Print confirmation with both stash hashes (if any):
    ```
    Switched to feature-auth.
      Stash for feature-auth: git stash apply --index <wt_hash>
      Stash for <previous-branch> (saved): git stash apply --index <main_hash>
    ```
    If `git stash apply --index` fails due to conflict: "Stash `<hash>` could not be applied cleanly. Run `git stash apply <hash>` to resolve manually (without --index)."

**If `feature-auth` is NOT in another worktree:** Skip worktree B steps (freeing, stashing worktree B). Only stash current directory if dirty.

**Stash lifecycle:** Stash commits are created via `git stash push`, tracked by hash in the WAL, and left in the global stash reflog after the swap completes. They are NOT dropped automatically — this ensures the stash commit objects remain protected from git GC (git only protects objects reachable from refs, not from arbitrary files like the WAL). The user applies stashes manually when ready. `uwt repair` cleans up stale `uwt:`-prefixed stash entries by scanning `git stash list` and dropping entries whose hashes are no longer referenced by any active WAL entry.

**Crash recovery:** If the process crashes at any point after the partial WAL write (step 5), the WAL contains stash commit hashes and progress. `uwt repair` (or the next `uwt` command) detects the incomplete swap and either completes or rolls back based on the recorded step.

### 8.2 Checkout to Worktree

**`uwt worktree create feature-auth`**:

1. If `feature-auth` is already checked out in a worktree: error with actionable suggestions.
2. Acquire lockfile. Re-validate preconditions.
3. Determine the worktree path: `~/.local/share/uwt/<encoded-main-path>/<encoded-branch-name>/`.
4. If target directory already exists (encoding collision): error with suggestion to use an explicit name.
5. If branch `feature-auth` doesn't exist, create it from the current branch (or `-b base`).
6. If branch exists but is not tracked in uwt state: adopt it — record the parent as the `-b` value if specified, otherwise default to the base branch. Warn: "Branch `feature-auth` exists but is not tracked by uwt. Adopting with parent `<parent>`."
7. Run `git worktree add <absolute-path> feature-auth`.
8. Record parent-child relationship and worktree path in state.
9. Release lockfile.
10. Print the worktree path.

**`uwt worktree create`** (no branch name):

1. Auto-generate a branch name using the configured strategy (default: `<current-branch>-YYYYMMDD-HHMM`, with prefix if configured).
2. Follow the same steps as above with the generated name.

**`uwt worktree remove [<branch>]`**:

1. If no branch specified: use current directory's branch.
2. If currently inside the worktree being removed: error with message — "Cannot remove the current worktree. Navigate out first: `cd $(uwt cd <main-branch>)`".
3. Acquire lockfile. Re-validate preconditions.
4. Run `git worktree remove <path>`.
5. Clean up associated `_uwt/*` temp branches.
6. Update state: mark branch as having no worktree (keep parent-child relationship).
7. Release lockfile.
8. Print confirmation.

### 8.3 Sync

**`uwt sync`**:

1. Run `git fetch` to update remote tracking branches (before lock — fetch doesn't modify uwt state).
2. Acquire lockfile. Verify all worktrees in the branch graph are clean. Abort with a message identifying dirty worktrees if any are found.
3. **Write sync intent to WAL**: snapshot all branch refs for abort recovery.
4. **Update base branch**: fast-forward the base branch to its remote tracking branch. First check for divergence: `git merge-base --is-ancestor <base> origin/<base>`. If not an ancestor (base has diverged from remote), warn and continue with local state — do not update. If fast-forwardable: if checked out in a worktree, `git -C <worktree-path> merge --ff-only origin/<base>`; if not checked out, `git branch -f <base> origin/<base>`.
5. Walk the branch graph starting from the configured base branch. If there are no child branches, print "All branches up to date" and proceed to step 7.
   - **Skip** branches currently on a `_uwt/hold/` temp branch (transient swap state).
   - For each child branch with an outdated parent: rebase onto updated parent.
     - **Branch in a worktree:** `git -C <worktree-path> rebase <parent>`. (Precondition: the branch must be checked out in that worktree, not a hold branch.)
     - **Branch not in any worktree:** Create temporary worktree `_uwt/rebase/<branch>`, rebase there. Remove if clean.
   - Cascade: if a parent was rebased, continue to its children.
6. **Conflict handling:** If a conflict occurs during rebase:
   - Stop the cascade for that sub-tree (children of the conflicted branch are blocked).
   - Continue syncing independent parallel sub-trees.
   - Record cascade progress in state: completed branches, conflicted branch, pending branches.
   - **Release lockfile.** (WAL preserves all state needed to resume.)
   - Report the conflict location and instructions for resolution (`uwt sync --continue` / `--abort`).
7. If sync completes with no conflicts: clear WAL. Release lockfile.

**`uwt sync --continue`**:

1. Acquire lockfile.
2. Read incomplete sync state from WAL.
3. Verify the conflicted rebase has been resolved.
4. Resume the cascade from where it stopped.
5. Clear sync WAL.
6. If the sync was triggered by a parent operation (e.g., `merge --all`): resume the parent operation (lock remains held — the parent inherits the lock and releases it when complete).
7. If no parent operation: release lockfile.

**`uwt sync --abort`**:

1. Acquire lockfile.
2. Read pre-sync branch refs from WAL.
3. For any `_uwt/rebase/*` temporary worktree with a rebase in progress: `git -C <path> rebase --abort`.
4. Remove any `_uwt/rebase/*` temporary worktrees created during sync (`git worktree remove`).
5. Reset rebased branches to their pre-sync positions:
   - **Branch checked out in a worktree:** `git -C <worktree-path> reset --hard <saved-ref>` (cannot use `git branch -f` on a checked-out branch).
   - **Branch not checked out:** `git branch -f <branch> <saved-ref>`.
6. If the sync has a parent `merge --all` operation: clear the parent WAL entry as well (stopping the merge sequence). Previously completed merges in the sequence are NOT rolled back.
7. Clear sync WAL. Release lockfile.

### 8.4 Write-Ahead Log

Mutating operations (checkout swap, sync, merge) record their intent and progress in the state file before performing destructive steps. This enables crash recovery and `--abort`.

**Tracked state:**
- **Operation ID**: Unique identifier for the operation.
- **Parent operation ID** (nullable): Links a child operation to the operation that spawned it. Used by `merge --all` to track which merge in the sequence triggered a grandchild sync, so that after `uwt sync --continue` completes, the `merge --all` sequence can resume with the next child.
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
- When `merge --all` triggers a grandchild sync, the merge WAL entry is cleared after the merge steps complete (branch deleted, worktree removed, state cleaned). The grandchild sync writes its own WAL entry with a parent operation ID pointing back to the `merge --all` entry.
- When `uwt sync --continue` resolves the grandchild conflict and completes, it reads the parent operation ID and resumes the `merge --all` sequence with the next child.
- When `uwt sync --abort` is run on a sync with a parent `merge --all`: both the sync WAL and the parent merge --all WAL are cleared. The abort stops the entire sequence. Previously completed merges are not rolled back.
- This is a two-level link only (merge → sync). No deeper nesting is supported.

**Recovery:** `uwt repair` or the next `uwt` command detects pending operations and either completes or rolls back based on recorded progress.

### 8.5 Merge

**`uwt merge feature-auth`**:

1. Acquire lockfile. Re-validate preconditions.
2. Verify preconditions:
   - Current branch is the parent of `feature-auth`.
   - Both branches are clean.
   - All grandchild worktrees (if any) are clean.
   - Strategy-dependent sync check:
     - **Fast-forward**: `feature-auth` must be fast-forwardable into current branch (i.e., current branch tip is an ancestor of `feature-auth` tip). If not, error: "run `uwt sync` first."
     - **Merge-commit / squash**: No divergence precondition — divergence is expected and handled by the merge strategy.
3. **Prompt confirmation** (skippable with `--yes`).
4. **Write pre-merge snapshot to WAL**: current branch ref, child branch ref, grandchild parent mappings, worktree paths.
5. **Merge:** `git merge --ff-only feature-auth` (or configured strategy: `--no-ff` for merge-commit, `--squash` for squash).
6. If merge fails: auto-rollback (clear WAL, no state was changed). Error with details.
7. **Re-parent grandchildren** in state: if `feature-auth` has children, update their parent to the current branch. (After merge succeeds, before deleting the merged branch.)
8. **Delete merged branch:** `git branch -d feature-auth`.
9. **Remove worktree** (if any): `git worktree remove <path>`.
10. **Clean up `_uwt/*` temp branches** associated with `feature-auth`.
11. **Remove state entries** for `feature-auth`.
12. **Clear merge WAL entry.** The merge itself is now complete.
13. **Sync grandchildren:** If re-parented children exist, internally run sync on them (rebase onto updated parent — should be clean since parent now contains the merged commits). This internal sync skips the cleanliness check (already verified in step 2, and lock has been held since). This sync writes its own WAL entry (with parent operation ID if part of `merge --all`). If this sync encounters a conflict, the lock is released (per §5.6) and the user resolves with `uwt sync --continue`.
14. Release lockfile (if not already released by a conflict pause in step 13).

**`uwt merge --all`**:

1. Determine the list of direct children to merge. If the list is empty: no-op, print "No children to merge."
2. **Single up-front confirmation prompt** listing all children to be merged (skippable with `--yes`). This replaces per-child prompts.
3. Acquire lockfile.
4. Write a `merge --all` WAL entry with the full list and progress tracker.
5. Execute each merge sequentially (steps 2, 4–13 of the single merge above for each child — step 1 lock acquisition, step 3 confirmation, and step 14 lock release are skipped since merge --all owns the lock lifecycle; precondition checks in step 2 still run for each child). After each child's step 12 (clear individual merge WAL), update the merge --all WAL progress to mark that child as completed before proceeding to the next child.
6. If a grandchild sync conflicts: stop the sequence (lock released per §5.6). After the user resolves with `uwt sync --continue`, the remaining merges resume automatically (via the parent operation link in §8.4).
7. If a merge itself fails (precondition not met): stop the sequence. Clear the `merge --all` WAL entry. Release lockfile. Previously completed merges are not rolled back. Print which children were merged and which remain.
8. On successful completion of all children: clear the `merge --all` WAL entry. Release lockfile.

**`uwt merge --abort`**:

1. Acquire lockfile.
2. Read pre-merge snapshot from WAL.
3. Reset current branch to pre-merge ref.
4. Recreate the child branch at its saved ref.
5. Recreate the worktree if it was removed.
6. Restore grandchild parent mappings in state.
7. Clean up any `_uwt/*` branches created during the merge.
8. Clear WAL. Release lockfile.

Note: `--abort` is only available while the merge WAL entry is active (before step 12 clears it). Once the merge WAL is cleared, the merge is **permanent** — the child branch is deleted and the parent branch has advanced. If the subsequent grandchild sync is aborted (`uwt sync --abort`), only the grandchild rebase is undone; the merge itself is not reversed. This applies to both single merge and `merge --all`.

### 8.6 In-Progress Operation Detection

Similar to git's `MERGE_HEAD` and `REBASE_MERGE` indicators, the WAL serves as an in-progress operation indicator.

- Any mutating `uwt` command checks the WAL before starting.
- If an operation is in progress: **block** with a clear message.

```
$ uwt merge feature-api
Error: a sync operation is in progress (conflict in feature-auth).
  Run 'uwt sync --continue' after resolving conflicts.
  Run 'uwt sync --abort' to restore pre-sync state.
  Run 'uwt status' for details.
```

- `uwt status` reports in-progress operations prominently, including parent operation context (e.g., "sync in progress, triggered by merge --all — 2 of 4 children merged").
- Read-only commands (`list`, `graph`, `cd`) are never blocked.

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
| devenv port conflicts | Multiple `devenv up` instances compete for ports | devenv supports `ports.<name>.allocate` for dynamic allocation; this is a per-repo config concern, not uwt's scope |
| Claude Code background tasks in worktrees ([#13087]) | Background tasks may fail to detect git repo | Agents own their worktree sessions atomically; not a practical concern |
| Git ref naming constraint | Cannot have both `feature-auth` (file) and `feature-auth/x` (directory) in `.git/refs/heads/` | Branch naming convention limits `/` to a single prefix level only; `uwt repair` detects conflicting `_uwt` branches |
| Directory encoding collisions | Different branch names or paths could theoretically produce the same encoded directory | Best-effort encoding with collision detection and user-facing error |
| Git stash is repository-global | `refs/stash` is shared across all worktrees; no per-worktree stash isolation | Stash commits tracked by hash in WAL; left in reflog to protect from GC; `uwt repair` cleans up stale entries |
| External git operations not blocked by lockfile | Users or other tools can modify the repo while uwt holds its lock | uwt only locks its own state; git-level races are detected reactively and surfaced with `uwt repair` suggestion |
| `git worktree list` main worktree detection | The first entry in `git worktree list` is the main working tree; explicit "main" label only available since git 2.36+ | Implementation should use the first entry's path, not rely on label text |

## 11. Open Decisions (Implementation Phase)

| Decision | Options | Notes |
|----------|---------|-------|
| Implementation language | Rust, Go, Zig, shell script | Rust has strong git ecosystem (gitoxide); Go is simpler; shell is fastest to prototype |
| State file format | JSON vs TOML | Functionally equivalent at this scale |
| State storage architecture | Single file vs distributed files | Single file is simpler; distributed (per-branch, per-operation) may scale better |
| Path/branch encoding algorithm | Must be deterministic and best-effort collision-resistant | Both main worktree paths and branch names use the same encoding |
| Shell integration for `uwt cd` | Shell function wrapper vs subshell | `cd $(uwt cd branch)` works but a shell function could be smoother |
| Default naming strategy | `timestamp` (default) vs `random` | Timestamp is sortable; random is more memorable |
| Operation ID generation | UUID, counter, timestamp-based | Must be unique within the state file lifetime |
| `uwt stash` subcommand | Future enhancement | Branch-aware stash listing (maps stash hashes to branches via WAL/state metadata). Not required for v1. |

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

