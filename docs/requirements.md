# Workflow Requirements (Draft v0.1)

This document augments `tought.md` and captures the planning requirements before committing to a specific implementation.

## Goal

Define a practical, local-first workflow for managing multiple parallel workstreams (including agent-driven work) with low context-switch cost and clear branch/workspace ownership.

## Core Decisions Captured

- The primary planning output is a thorough requirement set, not a final technical design.
- "Same branch in multiple directories" is not a required capability for v1.
- Preferred model: branch off from a parent/root branch and provide a convenient, safe path to integrate changes back.
- Integration conflict handling can follow standard Git workflows and known practices.
- The workflow should work fully on a local machine; remote/PR integration is optional and secondary.

## Functional Requirements

1. The system must define one canonical primary workspace path for daily interactive work (IDE-attached).
2. The system must support creating a child branch from the currently active branch in the primary workspace.
3. The system must support creating a child branch in a secondary workspace (worktree or clone mode).
4. The system must track explicit parent-child branch relationships as workflow metadata.
5. The system must provide a command to list active branches, their parent, and their workspace location.
6. The system must provide a command to switch/promote a chosen branch into the primary workspace.
7. The system must provide a command to integrate child changes into parent using policy-driven strategies (merge/rebase/cherry-pick).
8. The system must detect integration conflicts and support resumable workflows after manual conflict resolution.
9. The system must operate fully locally without requiring push or pull request creation.
10. The system must protect uncommitted work before risky operations (stash/snapshot/checkpoint behavior).
11. The system must show branch state signals: clean/dirty, ahead/behind parent, merge-ready/conflicted.
12. The system must support closing a child branch with safe cleanup of workspace and related metadata.
13. The system must support agent-friendly branch workflows (clear create/work/integrate lifecycle).
14. The system must enforce deterministic folder conventions for spawned workspaces.
15. The system must provide recovery/repair operations for moved/deleted workspace paths.
16. The system must provide machine-readable status output in addition to human-readable output.
17. The system must allow configurable default integration strategies.
18. The system must visualize branch relationships as a stack/graph (at least textual).
19. The system must not depend on checking out the same branch in multiple directories.
20. The system should optionally support detached inspection/testing workspaces.

## Non-Functional Requirements

- Low cognitive overhead for frequent context switching.
- Safety-first defaults with minimal destructive behavior.
- Deterministic and scriptable command behavior for automation and sub-agents.
- Idempotent operations where possible.
- Compatibility with standard Git conflict-resolution habits.

## Out of Scope (Current Phase)

- Requiring full support for the same branch checked out in multiple directories.
- Replacing native Git conflict semantics with a custom conflict engine.
- Building full remote orchestration and PR lifecycle tooling in v1.

## Assumptions

- Users are comfortable resolving merge/rebase conflicts with standard Git tools.
- Sub-agents can follow branch lifecycle instructions if the workflow contract is explicit.
- Most work happens in one primary workspace, with occasional branch-specific secondary workspaces.

## Acceptance Criteria for Planning Phase

This requirements phase is complete when:

1. Team agrees on primary workflow model (single primary workspace + branch-off + integrate-back).
2. Team agrees that same-branch multi-directory support is non-required for v1.
3. Functional requirements are approved as sufficient for implementation design.
4. Open decisions are documented and prioritized.

## Open Decisions

1. Preferred default integration strategy (merge-first vs rebase-first).
2. Metadata storage location and schema (e.g., `.git/...` vs repo file).
3. Workspace naming convention and lifecycle policy (retention/cleanup).
4. Recovery policy for abandoned or stale workspaces.
5. Minimum command set for v1 CLI.
