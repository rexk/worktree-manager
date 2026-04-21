# wkm — Worktree Manager

[![CI](https://github.com/rexk/worktree-manager/actions/workflows/ci.yml/badge.svg)](https://github.com/rexk/worktree-manager/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A CLI tool that manages git worktrees with parent-child branch relationships, cascade rebase, and crash-safe state management.

## Quick Start

```bash
# Initialize wkm in your repo
wkm init --base main

# Create a feature branch (child of current branch) and switch to it
wkm checkout -b feature-auth

# Create a worktree for parallel work on another branch
wkm worktree create feature-ui

# See the branch tree
wkm graph
# main
# ├── feature-auth  ~/dev/wkm/feature-auth/
# └── feature-ui    ~/dev/wkm/feature-ui/

# Rebase all child branches onto their updated parents
wkm sync

# Merge a feature back into its parent
wkm merge feature-auth
```

## Installation

### Prebuilt binaries (Linux / macOS)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/rexk/worktree-manager/main/install.sh | sh
```

Options:

```bash
# Install a specific version
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/rexk/worktree-manager/main/install.sh | sh -s -- --tag v0.1.0

# Install to a custom directory
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/rexk/worktree-manager/main/install.sh | sh -s -- --to ~/bin
```

### From source

Requires **Rust 1.85+** and **git** on your PATH.

```bash
cargo install --git https://github.com/rexk/worktree-manager wkm-cli
```

## Shell Setup

Add the shell wrapper to get `wkm wp <branch>` (jump to a branch's worktree directory):

```bash
# bash / zsh — add to your .bashrc or .zshrc
eval "$(wkm shell-setup)"

# fish — add to your config.fish
wkm shell-setup --shell fish | source
```

Generate completions:

```bash
wkm completions bash > ~/.local/share/bash-completion/completions/wkm
wkm completions zsh  > ~/.local/share/zsh/site-functions/_wkm
wkm completions fish > ~/.config/fish/completions/wkm.fish
```

## Commands

### Core

| Command | Alias | Description |
|---------|-------|-------------|
| `wkm init` | | Initialize worktree tracking for the current repo |
| `wkm checkout <branch>` | `co` | Switch current directory to the specified branch |
| `wkm checkout -b <branch>` | `co -b` | Create a new child branch and switch to it |
| `wkm adopt <branch>` | | Adopt an existing git branch into wkm tracking |
| `wkm worktree create [<branch>]` | `wt create` | Create a worktree for a branch |
| `wkm worktree remove [<branch>]` | `wt rm` | Remove a branch's worktree (keeps the branch) |
| `wkm sync` | | Cascade rebase all child branches onto updated parents |
| `wkm merge <branch>` | | Merge a child branch into the current branch |

### Visibility

| Command | Alias | Description |
|---------|-------|-------------|
| `wkm list` | `ls` | Show all tracked branches with state signals |
| `wkm graph` | | ASCII branch dependency tree |
| `wkm status [<branch>]` | | Detailed state for a branch |
| `wkm wp <branch>` | | Output (or cd to) a branch's worktree path |

### Maintenance

| Command | Description |
|---------|-------------|
| `wkm config [key] [value]` | View or set repo-level configuration |
| `wkm repair` | Reconcile wkm state with git/filesystem |
| `wkm drop <branch>` | Remove a branch from wkm tracking |
| `wkm stash list\|apply\|drop` | Manage stashes captured during branch swaps |

## Key Features

### Branch Tracking

wkm records parent-child relationships between branches in `.git/wkm.toml`. When you create a branch with `wkm checkout -b`, the current branch is automatically recorded as the parent. This relationship drives cascade rebase, merge, and the branch graph visualization.

### Cascade Rebase

`wkm sync` fetches from the remote and rebases every tracked branch onto its updated parent in topological order. A chain like `main → feature → sub-feature` is rebased bottom-up so each branch stays current. If a conflict occurs, the operation pauses for resolution and can be continued with `wkm sync --continue` or rolled back with `wkm sync --abort`.

### Checkout Swap

When you `wkm checkout` a branch that is checked out in another worktree, wkm automatically stashes both worktrees, swaps the branches using a temporary hold branch, and restores the stashes. Dirty state (staged, unstaged, and optionally untracked files) is preserved across the swap.

### Crash Safety

All multi-step operations write a WAL (write-ahead log) entry before mutating state. If wkm crashes mid-operation, `wkm repair` replays or rolls back incomplete operations. State writes are atomic (tempfile + rename), and a PID-based lock file prevents concurrent access.

### Local-First

wkm works entirely with your local git repo. There is no server, no account, and no network requirement beyond what git itself needs. All state lives in `.git/wkm.toml` inside your repository.

## How It Works

- **State file**: `.git/wkm.toml` stores branch entries (parent, secondary worktree path if any, stash commit, metadata) with schema versioning. Branches hosted in the main worktree don't carry a stored worktree path — main-worktree hosting is inferred at runtime via `git worktree list`.
- **Worktree storage**: Worktrees are created under `~/.local/share/wkm/` by default (configurable via `WKM_DATA_DIR` or per-repo config). The directory is named after the repo so your terminal prompt stays meaningful.
- **Git interaction**: wkm shells out to the `git` CLI for all git operations — no libgit2 dependency.
- **Architecture**: Three crates — `wkm-core` (business logic), `wkm-cli` (Clap wrapper), `wkm-sandbox` (test fixtures).

See [docs/SPEC.md](docs/SPEC.md) for the full functional specification.

## License

[MIT](LICENSE)
