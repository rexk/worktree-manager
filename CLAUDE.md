# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build                          # Debug build
cargo build --release                # Release build
cargo test                           # All tests
cargo test -p wkm-core               # Core library tests only
cargo test -p wkm-core ops::sync     # Single test module
cargo test -- --nocapture            # Show stdout in tests
cargo clippy                         # Lint
cargo fmt --check                    # Check formatting
cargo run -p wkm-cli -- <command>    # Run the CLI
```

## Architecture

**wkm** (Worktree Manager) is a Rust CLI tool that manages git worktrees with parent-child branch relationships, cascade rebase, and crash-safe state management.

### Workspace Crates

- **wkm-core**: All business logic (~6K lines). Operations, git abstraction, state persistence, branch graph utilities.
- **wkm-cli**: Thin Clap wrapper that dispatches to `wkm-core` operations.
- **wkm-sandbox**: Test fixture (`TestRepo`) that creates temporary git repos for integration tests.

### Core Module Layout (`wkm-core/src/`)

- **`ops/`** — High-level operations (init, checkout, sync, merge, worktree create/remove, adopt, stash, repair, list, status). Each file is one operation.
- **`git/`** — Trait-based git abstraction (`mod.rs` defines traits, `cli.rs` shells out to `git`, `types.rs` has data types). All git interaction goes through these traits.
- **`state/`** — TOML state file (`wkm.toml` in `.git/`) with atomic writes, PID-based file locking (`lock.rs`), and a write-ahead log (WAL) for crash recovery.
- **`repo.rs`** — `RepoContext`: resolves `.git` dir, main worktree path, state file path, and storage directory from any worktree location.
- **`graph.rs`** — Branch relationship graph: `children_of`, `descendants_of`, `topo_sort`, `ascii_tree`.
- **`encoding.rs`** — Path hashing (`hash_path`) and random worktree ID generation (`generate_worktree_id`).
- **`error.rs`** — All error types via `thiserror`.

### Key Design Decisions

- **Git CLI shell-out** (not libgit2) for compatibility and simplicity.
- **State file**: `.git/wkm.toml` stores branch entries (parent, worktree path, stash commit, metadata) with schema versioning.
- **Atomic state writes**: tempfile + rename to prevent corruption.
- **WAL**: write-ahead log entries allow crash recovery of multi-step operations.
- **Swap operation** in checkout: moves a checked-out branch between worktrees.
- **Cascade rebase** in sync: topologically sorts descendants and rebases each onto its updated parent.
- **Storage directory**: `<storage-dir>/<random-worktree-id>/<repo-name>/` for worktrees. `config.storage_dir` stores the fully resolved path (including the repo hash). When unset, computed from: `WKM_DATA_DIR` env → `XDG_DATA_HOME/wkm/` → `~/.local/share/wkm/`, with `<hashed-repo-path>` appended. `<repo-name>` is the last component of the main worktree path (so the terminal prompt shows the repo name).

### Testing

Tests use `wkm-sandbox::TestRepo` which creates a temporary bare repo + worktree.

## Documentation

`docs/SPEC.md` is the authoritative 48KB functional specification covering goals, workflows, and design decisions.
