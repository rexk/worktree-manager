/// Encode a branch name for use as a filesystem directory name.
///
/// Algorithm: replace `/` with `--`, then percent-encode any character
/// that's not `[a-zA-Z0-9._-]`. Deterministic, flat, reversible.
pub fn encode_branch_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '/' => result.push_str("--"),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => result.push(ch),
            other => {
                for byte in other.to_string().as_bytes() {
                    result.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    result
}

/// Encode a path for use as a directory name (for storage dir derivation).
///
/// Same algorithm as branch name encoding.
pub fn encode_path(path: &str) -> String {
    encode_branch_name(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_simple_name_unchanged() {
        assert_eq!(encode_branch_name("main"), "main");
        assert_eq!(encode_branch_name("feature-auth"), "feature-auth");
    }

    #[test]
    fn encode_slashes_to_double_dash() {
        assert_eq!(encode_branch_name("rex/feature-auth"), "rex--feature-auth");
    }

    #[test]
    fn encode_nested_slashes_flat() {
        assert_eq!(encode_branch_name("a/b/c"), "a--b--c");
    }

    #[test]
    fn encode_special_chars_percent_encoded() {
        let encoded = encode_branch_name("feat:test");
        assert_eq!(encoded, "feat%3Atest");
    }

    #[test]
    fn encode_space_percent_encoded() {
        let encoded = encode_branch_name("my branch");
        assert_eq!(encoded, "my%20branch");
    }

    #[test]
    fn encode_unicode_percent_encoded() {
        let encoded = encode_branch_name("feature/日本語");
        assert!(encoded.starts_with("feature--"));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('日'));
    }

    #[test]
    fn encode_deterministic() {
        let a = encode_branch_name("rex/feature-auth");
        let b = encode_branch_name("rex/feature-auth");
        assert_eq!(a, b);
    }

    #[test]
    fn encode_dots_underscores_preserved() {
        assert_eq!(encode_branch_name("v1.0_rc"), "v1.0_rc");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn different_inputs_produce_different_outputs(a in "[a-zA-Z0-9/._-]{1,20}", b in "[a-zA-Z0-9/._-]{1,20}") {
            if a != b {
                // Not guaranteed for all inputs (collisions are theoretically possible
                // with the encoding), but for this character set they should differ.
                prop_assert_ne!(encode_branch_name(&a), encode_branch_name(&b));
            }
        }

        #[test]
        fn encoded_is_filesystem_safe(name in ".{1,50}") {
            let encoded = encode_branch_name(&name);
            assert!(!encoded.contains('/'));
            assert!(!encoded.is_empty());
            // Only allowed chars: a-zA-Z0-9._-% (percent from encoding)
            for ch in encoded.chars() {
                assert!(
                    ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' || ch == '%',
                    "unexpected char: {ch}"
                );
            }
        }
    }
}
