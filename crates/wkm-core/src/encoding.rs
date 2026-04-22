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

pub const MAX_ALIAS_LENGTH: usize = 32;

/// Validate a workspace alias. Aliases are:
/// - non-empty, at most 32 chars
/// - match `^[a-z0-9][a-z0-9_-]*$` (lowercase ASCII alphanumerics plus `_` and `-`)
/// - must not start with `@` (reserved for built-in tokens like `@main`)
///
/// The bare word `main` is permitted as a user alias; it is resolved via the
/// workspace map and does not shadow the built-in `@main` token.
pub fn validate_workspace_alias(alias: &str) -> Result<(), String> {
    if alias.is_empty() {
        return Err("workspace alias cannot be empty".to_string());
    }
    if alias.len() > MAX_ALIAS_LENGTH {
        return Err(format!(
            "workspace alias '{alias}' is too long (max {MAX_ALIAS_LENGTH} chars)"
        ));
    }
    if alias.starts_with('@') {
        return Err(format!(
            "workspace alias '{alias}' cannot start with '@' (reserved for built-in tokens)"
        ));
    }
    let mut chars = alias.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "workspace alias '{alias}' must start with a lowercase letter or digit"
        ));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return Err(format!(
                "workspace alias '{alias}' contains invalid character '{c}' (allowed: a-z 0-9 _ -)"
            ));
        }
    }
    Ok(())
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

    #[test]
    fn validate_alias_accepts_valid_names() {
        assert!(validate_workspace_alias("specs").is_ok());
        assert!(validate_workspace_alias("bugfix-3").is_ok());
        assert!(validate_workspace_alias("feat_a").is_ok());
        assert!(validate_workspace_alias("9lives").is_ok());
        // bare `main` is a permitted user alias (it does not shadow `@main`).
        assert!(validate_workspace_alias("main").is_ok());
    }

    #[test]
    fn validate_alias_rejects_empty() {
        assert!(validate_workspace_alias("").is_err());
    }

    #[test]
    fn validate_alias_rejects_at_prefix() {
        assert!(validate_workspace_alias("@main").is_err());
        assert!(validate_workspace_alias("@specs").is_err());
    }

    #[test]
    fn validate_alias_rejects_uppercase() {
        assert!(validate_workspace_alias("Specs").is_err());
    }

    #[test]
    fn validate_alias_rejects_bad_start() {
        assert!(validate_workspace_alias("-leading-dash").is_err());
        assert!(validate_workspace_alias("_leading").is_err());
    }

    #[test]
    fn validate_alias_rejects_bad_chars() {
        assert!(validate_workspace_alias("foo/bar").is_err());
        assert!(validate_workspace_alias("foo bar").is_err());
        assert!(validate_workspace_alias("foo.bar").is_err());
    }

    #[test]
    fn validate_alias_rejects_too_long() {
        let long = "a".repeat(MAX_ALIAS_LENGTH + 1);
        assert!(validate_workspace_alias(&long).is_err());
        let exact = "a".repeat(MAX_ALIAS_LENGTH);
        assert!(validate_workspace_alias(&exact).is_ok());
    }
}
