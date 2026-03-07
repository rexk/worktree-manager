# Contributing to wkm

Thanks for your interest in contributing!

## Development

```bash
cargo build                  # Debug build
cargo test                   # Run all tests
cargo clippy -- -D warnings  # Lint (must pass in CI)
cargo fmt --check            # Format check (must pass in CI)
```

Requires Rust 1.85+ (edition 2024) and git installed on your PATH.

## Architecture

wkm is a Cargo workspace with three crates:

- **wkm-core** — All business logic: operations, git abstraction, state persistence, branch graph utilities.
- **wkm-cli** — Thin Clap wrapper that dispatches to `wkm-core` operations.
- **wkm-sandbox** — Test fixture (`TestRepo`) that creates temporary git repos for integration tests.

See [CLAUDE.md](CLAUDE.md) for a detailed module map and [docs/SPEC.md](docs/SPEC.md) for the full functional specification.

## Pull Requests

1. Fork the repo and create a branch from `main`.
2. Make your changes. Add tests for new features.
3. Ensure all CI checks pass (`fmt`, `clippy`, `test`).
4. Open a pull request with a clear description of what changed and why.
