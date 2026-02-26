use std::collections::BTreeMap;

use crate::state::types::BranchEntry;

/// Get the children of a branch from the state.
pub fn children_of<'a>(
    parent: &str,
    branches: &'a BTreeMap<String, BranchEntry>,
) -> Vec<(&'a String, &'a BranchEntry)> {
    branches
        .iter()
        .filter(|(_, entry)| entry.parent.as_deref() == Some(parent))
        .collect()
}

/// Get all descendants of a branch (children, grandchildren, etc.) in BFS order.
pub fn descendants_of<'a>(
    root: &str,
    branches: &'a BTreeMap<String, BranchEntry>,
) -> Vec<(&'a String, &'a BranchEntry)> {
    let mut result = Vec::new();
    let mut queue = vec![root.to_string()];

    while let Some(current) = queue.first().cloned() {
        queue.remove(0);
        let kids = children_of(&current, branches);
        for (name, _) in &kids {
            queue.push((*name).clone());
        }
        result.extend(kids);
    }
    result
}

/// Get ancestors of a branch (parent, grandparent, etc.) up to root.
pub fn ancestors_of(branch: &str, branches: &BTreeMap<String, BranchEntry>) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = branch.to_string();

    while let Some(entry) = branches.get(&current) {
        if let Some(ref parent) = entry.parent {
            result.push(parent.clone());
            current = parent.clone();
        } else {
            break;
        }
    }
    result
}

/// Topological sort of branches rooted at `root` (root-first, leaves-last).
/// Useful for sync operations where parents must be rebased before children.
pub fn topo_sort(root: &str, branches: &BTreeMap<String, BranchEntry>) -> Vec<String> {
    let mut result = Vec::new();
    let mut stack = vec![root.to_string()];

    while let Some(current) = stack.pop() {
        result.push(current.clone());
        let kids = children_of(&current, branches);
        for (name, _) in kids.into_iter().rev() {
            stack.push(name.clone());
        }
    }
    result
}

/// Build an ASCII tree representation.
pub fn ascii_tree(
    root: &str,
    branches: &BTreeMap<String, BranchEntry>,
    annotate: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut lines = Vec::new();
    build_tree_lines(root, branches, annotate, &mut lines, "", true);
    lines.join("\n")
}

fn build_tree_lines(
    name: &str,
    branches: &BTreeMap<String, BranchEntry>,
    annotate: &dyn Fn(&str) -> Option<String>,
    lines: &mut Vec<String>,
    prefix: &str,
    is_last: bool,
) {
    let connector = if lines.is_empty() {
        ""
    } else if is_last {
        "└── "
    } else {
        "├── "
    };

    let annotation = annotate(name)
        .map(|a| format!(" ({a})"))
        .unwrap_or_default();
    lines.push(format!("{prefix}{connector}{name}{annotation}"));

    let children = children_of(name, branches);
    let child_prefix = if lines.len() == 1 {
        String::new()
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };

    for (i, (child_name, _)) in children.iter().enumerate() {
        let child_is_last = i == children.len() - 1;
        build_tree_lines(
            child_name,
            branches,
            annotate,
            lines,
            &child_prefix,
            child_is_last,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::BranchEntry;

    fn make_entry(parent: Option<&str>) -> BranchEntry {
        BranchEntry {
            parent: parent.map(|s| s.to_string()),
            worktree_path: None,
            stash_commit: None,
            description: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            previous_branch: None,
        }
    }

    fn sample_tree() -> BTreeMap<String, BranchEntry> {
        let mut branches = BTreeMap::new();
        branches.insert("main".to_string(), make_entry(None));
        branches.insert("feature-a".to_string(), make_entry(Some("main")));
        branches.insert("feature-b".to_string(), make_entry(Some("main")));
        branches.insert("sub-a1".to_string(), make_entry(Some("feature-a")));
        branches.insert("sub-a2".to_string(), make_entry(Some("feature-a")));
        branches
    }

    #[test]
    fn children_of_root() {
        let branches = sample_tree();
        let kids = children_of("main", &branches);
        let names: Vec<&str> = kids.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["feature-a", "feature-b"]);
    }

    #[test]
    fn children_of_leaf() {
        let branches = sample_tree();
        let kids = children_of("sub-a1", &branches);
        assert!(kids.is_empty());
    }

    #[test]
    fn descendants_bfs() {
        let branches = sample_tree();
        let desc = descendants_of("main", &branches);
        let names: Vec<&str> = desc.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["feature-a", "feature-b", "sub-a1", "sub-a2"]);
    }

    #[test]
    fn ancestors_chain() {
        let branches = sample_tree();
        let ancs = ancestors_of("sub-a1", &branches);
        assert_eq!(ancs, vec!["feature-a", "main"]);
    }

    #[test]
    fn topo_sort_order() {
        let branches = sample_tree();
        let sorted = topo_sort("main", &branches);
        // Parents before children
        assert_eq!(
            sorted,
            vec!["main", "feature-a", "sub-a1", "sub-a2", "feature-b"]
        );
    }

    #[test]
    fn ascii_tree_structure() {
        let branches = sample_tree();
        let tree = ascii_tree("main", &branches, &|_| None);
        assert!(tree.contains("main"));
        assert!(tree.contains("feature-a"));
        assert!(tree.contains("sub-a1"));
    }

    #[test]
    fn ascii_tree_with_annotations() {
        let branches = sample_tree();
        let tree = ascii_tree("main", &branches, &|name| {
            if name == "feature-a" {
                Some("dirty".to_string())
            } else {
                None
            }
        });
        assert!(tree.contains("feature-a (dirty)"));
    }
}
