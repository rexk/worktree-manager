use std::io::IsTerminal;

use console::Style;

/// Returns true when both stdin and stderr are TTYs, meaning we can show
/// interactive prompts (dialoguer writes to stderr by default).
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Shared style palette for colored output.
/// When stdout is not a TTY or NO_COLOR is set, `console` automatically
/// strips ANSI codes, so callers don't need to check.
pub struct Styles {
    pub branch: Style,
    pub parent: Style,
    pub ahead: Style,
    pub behind: Style,
    pub stash: Style,
    pub dirty: Style,
    pub in_progress: Style,
    pub tree_line: Style,
}

impl Styles {
    pub fn new() -> Self {
        Self {
            branch: Style::new().bold(),
            parent: Style::new().dim(),
            ahead: Style::new().green(),
            behind: Style::new().red(),
            stash: Style::new().yellow(),
            dirty: Style::new().red(),
            in_progress: Style::new().yellow(),
            tree_line: Style::new().dim(),
        }
    }
}
