use crate::error::WkmError;
use crate::graph;
use crate::repo::RepoContext;
use crate::state;

/// Render the branch graph as an ASCII tree.
pub fn render_graph(
    ctx: &RepoContext,
    annotate: &dyn Fn(&str) -> Option<String>,
) -> Result<String, WkmError> {
    let wkm_state = state::read_state(&ctx.state_path)?.ok_or(WkmError::NotInitialized)?;

    let root = &wkm_state.config.base_branch;
    Ok(graph::ascii_tree(root, &wkm_state.branches, annotate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::init::{self, InitOptions};
    use crate::state::types::BranchEntry;
    use wkm_sandbox::TestRepo;

    #[test]
    fn graph_ascii_tree() {
        let repo = TestRepo::new();
        let ctx = RepoContext::from_path(repo.path()).unwrap();
        init::init(&ctx, &InitOptions::default()).unwrap();

        let mut wkm_state = state::read_state(&ctx.state_path).unwrap().unwrap();
        wkm_state.branches.insert(
            "feature-a".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        wkm_state.branches.insert(
            "feature-b".to_string(),
            BranchEntry {
                parent: Some("main".to_string()),
                worktree_path: None,
                stash_commit: None,
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                previous_branch: None,
            },
        );
        state::write_state(&ctx.state_path, &wkm_state).unwrap();

        let tree = render_graph(&ctx, &|_| None).unwrap();
        assert!(tree.contains("main"));
        assert!(tree.contains("feature-a"));
        assert!(tree.contains("feature-b"));
    }
}
