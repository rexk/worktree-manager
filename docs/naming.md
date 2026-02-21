# Naming Reasoning: `wkm`

The project has been named **`wkm`** after a rigorous selection process focused on ergonomics, uniqueness, and platform compatibility.

## 1. Selection Criteria

- **Ergonomics**: Must be short (3 characters) and easy to type on QWERTY and alternative layouts.
- **Uniqueness**: Must not collide with standard system utilities on Arch Linux, Debian/Ubuntu, or macOS.
- **Context**: Must be mnemonic and clearly related to Git Worktrees.
- **Searchability**: Should not produce excessive noise when searching for issues or packages.

## 2. Why `wkm`?

### Collision-Free
A comprehensive check was performed across the following package managers and systems:
- **Arch Linux (pacman/AUR)**: No exact matches found.
- **Debian/Ubuntu (apt)**: No exact matches found.
- **macOS (Homebrew)**: No exact matches found.
- **General CLI**: Unlike `wt` (Windows Terminal) or `uwt` (Security/Proxy tools), `wkm` has no high-profile system-level collisions.

### Ergonomics
- `w`, `k`, and `m` are well-distributed across the keyboard, making it a very fast, satisfying sequence to type compared to more cramped alternatives like `gwt` or `uwt`.

### Meaning
The name **`wkm`** is primarily an ergonomic and unique command. It can be thought of as an abbreviation for **WorKtree Manager**. The central **`k`** provides a strong phonetic and visual anchor that differentiates it from more common but congested names like `wt` or `wtm`.

## 3. Rejected Alternatives

- **`wt`**: Heavily congested. Collides with Windows Terminal and at least five existing community worktree helpers.
- **`uwt`**: Collides with Whonix/Debian security tools.
- **`gwt`**: Multiple existing scripts and tools use this name.
- **`swap` / `shift`**: Reserved or too close to system memory/shell builtins.
- **`wkt`**: Collides mentally with "Well-Known Text" (GIS standard) and has minor package aliases in NPM.
