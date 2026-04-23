---
name: wkm-adopt
description: Bring existing plain-git branches under wkm tracking, inferring parents from merge-base. Use when the user asks to "adopt", "track existing branches", or "pull branches into wkm".
argument-hint: [branch...] [--parent <name>] [--all]
allowed-tools: Bash
---

# /wkm:adopt — track existing branches

`wkm adopt` records plain-git branches in `.git/wkm.toml`. With no parent given, wkm infers one from merge-base against the configured base branch. With `--all`, it adopts every untracked local branch it finds.

## Arguments

`$ARGUMENTS` may be any mix of: branch names, `--parent <name>`, `--all`.

- Branch names and `--all` are mutually exclusive (clap enforces this).
- `--parent` applies to whichever branches are being adopted.

Pass everything through to wkm verbatim after sanity-checking.

## Execution

1. Show current state: `wkm list` (which branches are already tracked). Skip this if the user just asked to adopt a specific branch they named.
2. Dispatch:
   - **`--all`** (optionally with `--parent`): `wkm adopt --all [--parent <p>]`.
   - **Explicit branches**: `wkm adopt <b1> <b2> ... [--parent <p>]`.
   - **No args**: wkm enters an interactive picker, which will not work from Claude. Instead:
     - Run `wkm adopt --all` after confirming with the user, **or**
     - Ask the user which branches they want to adopt.
3. Echo wkm's output. Each adopted branch prints `Adopted '<name>'`; skipped ones print `Skipped '<name>' (already tracked)`.
4. Run `wkm graph` after to show the inferred parents. If a parent looks wrong, suggest `wkm set-parent <branch> <new-parent>`.

## Notes

- wkm only adopts **local** branches. If the user wants to adopt `origin/foo`, they need to `git checkout -b foo origin/foo` first.
- wkm will not adopt a branch whose name starts with `_wkm/` — those are internal markers.
