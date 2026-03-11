/// Hash a path to an 8-character lowercase hex string for use as a storage directory name.
///
/// Uses SHA-256, taking the first 8 hex characters. Deterministic and filesystem-safe.
pub fn hash_path(path: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(path.as_bytes());
    format!("{:x}", hash).chars().take(8).collect()
}

/// Generate an 8-character random lowercase hex string for use as a worktree directory name.
pub fn generate_worktree_id() -> String {
    use rand::Rng;
    let bytes: [u8; 4] = rand::rng().random();
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_path_is_8_lowercase_hex() {
        let h = hash_path("/home/user/project");
        assert_eq!(h.len(), 8);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn hash_path_deterministic() {
        let a = hash_path("/home/user/project");
        let b = hash_path("/home/user/project");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_path_different_inputs_differ() {
        let a = hash_path("/home/user/project-a");
        let b = hash_path("/home/user/project-b");
        assert_ne!(a, b);
    }

    #[test]
    fn worktree_id_is_8_lowercase_hex() {
        let id = generate_worktree_id();
        assert_eq!(id.len(), 8);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn worktree_id_is_random() {
        let a = generate_worktree_id();
        let b = generate_worktree_id();
        // Statistically near-impossible to collide
        assert_ne!(a, b);
    }
}
