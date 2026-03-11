# /release — Prepare a version bump commit

Analyze conventional commits since the last release, determine the semver bump, update Cargo.toml, and commit. Does NOT push or create tags.

## Steps

### 1. Find the last release baseline

```bash
gh release view --json tagName,publishedAt 2>/dev/null
```

- If a release exists, use its tag as the baseline: `git log <tag>..HEAD --oneline`
- If no release exists (first release), use ALL commits: `git log --oneline`

### 2. Collect and display commits

Run the appropriate `git log` command and display the commits grouped by type:

- **Breaking** (`feat!:`, `fix!:`, or any type with `!`, or commit body contains `BREAKING CHANGE:`)
- **Features** (`feat:`)
- **Fixes** (`fix:`)
- **Other** (`refactor:`, `docs:`, `chore:`, `perf:`, `test:`, `ci:`, `build:`, `style:`)

### 3. Determine semver bump

Parse the current version from `workspace.package.version` in root `Cargo.toml`.

Apply these rules:
- **major**: any breaking change (but if current major is `0`, a breaking change bumps **minor** instead per semver 0.x convention)
- **minor**: any `feat:` commit (no breaking changes)
- **patch**: only `fix:`, `refactor:`, `perf:`, `docs:`, `chore:`, etc.

If there are no commits since the last release, stop and inform the user.

### 4. Present the recommendation

Show:
- Current version
- Recommended bump level and reasoning (list the commits that drive the decision)
- Proposed new version

Then ask the user to confirm or override:
- Accept the recommendation
- Choose a different bump level (major/minor/patch)
- Specify an exact version
- Specify a pre-release suffix (e.g., `0.2.0-rc.1`, `0.2.0-alpha.1`)
- Cancel

### 5. Update Cargo.toml and commit

After confirmation, update **two** version strings in root `Cargo.toml`:
1. `version = "X.Y.Z"` under `[workspace.package]`
2. `version = "X.Y.Z"` in the `wkm-core` line under `[workspace.dependencies]`

Then regenerate the lockfile:
```bash
cargo generate-lockfile
```

Then create a commit:
```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to X.Y.Z"
```

### 6. Show next steps

After the commit, display:
```
Version bumped to X.Y.Z

Next steps (manual):
  git tag vX.Y.Z
  git push origin main --tags    # triggers release workflow
```

## Edge cases

- **No conventional commit prefix**: Treat as `patch`-level (same as `chore:`).
- **Scoped commits** (e.g., `feat(core):`): Parse the type before the scope.
- **Multiple `!` commits**: Still one major bump.
- **Pre-release versions**: If current version has a pre-release suffix (e.g., `0.2.0-rc.1`), bumping to release strips the suffix → `0.2.0`. If user wants another pre-release, they specify explicitly.
