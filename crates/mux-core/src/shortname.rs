//! Shortname generation and sanitization for mux sessions.
//!
//! Spec: docs/02 §Shortname sanitisation
//!
//! A shortname is a human-readable label for a tmux session and SQLite row.
//! Rules:
//! - Characters: lowercase ASCII alphanumeric and hyphens only.
//! - Max length: 124 bytes (tmux limit with `mux-` prefix overhead).
//! - Truncation: at hyphen boundary where possible; hard-truncate at 124 otherwise.
//! - tmux session names use `mux-{shortname}` — the `mux-` prefix is NOT part of the
//!   stored shortname.

/// Maximum byte length of a stored shortname (without the `mux-` tmux prefix).
pub const MAX_SHORTNAME_BYTES: usize = 124;

/// Sanitize a component (repo leaf or branch name) for use in a shortname.
///
/// Rules: lowercase, replace any non-`[a-z0-9]` character with `-`, collapse
/// consecutive hyphens, strip leading and trailing hyphens.
pub fn sanitize_component(s: &str) -> String {
    let mut out = String::new();
    let mut prev_hyphen = false;
    for c in s.chars() {
        let c_lower = c.to_ascii_lowercase();
        if c_lower.is_ascii_alphanumeric() {
            out.push(c_lower);
            prev_hyphen = false;
        } else if !prev_hyphen {
            out.push('-');
            prev_hyphen = true;
        }
    }
    out.trim_matches('-').to_owned()
}

/// Truncate a shortname to [`MAX_SHORTNAME_BYTES`] bytes.
///
/// Prefers truncating at a hyphen boundary. If no hyphen exists within the first
/// 124 bytes, hard-truncates at 124 bytes.
pub fn truncate_shortname(s: &str) -> String {
    if s.len() <= MAX_SHORTNAME_BYTES {
        return s.to_owned();
    }
    let prefix = &s[..MAX_SHORTNAME_BYTES];
    match prefix.rfind('-') {
        Some(pos) if pos > 0 => prefix[..pos].to_owned(),
        _ => prefix.to_owned(),
    }
}

/// Generate a deterministic shortname for a non-main branch.
///
/// Format: `{sanitized_repo_leaf}-{sanitized_branch}` truncated to 124 bytes.
///
/// Same `repo_leaf` + `branch` input always produces the same output (stable).
/// Collision with existing shortnames is resolved by the caller (append `-2`, `-3`, etc.).
pub fn shortname_for_branch(repo_leaf: &str, branch: &str) -> String {
    let leaf = sanitize_component(repo_leaf);
    let branch_s = sanitize_component(branch);
    if leaf.is_empty() && branch_s.is_empty() {
        return truncate_shortname("session");
    }
    let combined = if leaf.is_empty() {
        branch_s
    } else if branch_s.is_empty() {
        leaf
    } else {
        format!("{leaf}-{branch_s}")
    };
    truncate_shortname(&combined)
}

/// Generate a shortname for the main/master branch with a given adjective+noun pair.
///
/// Format: `{sanitized_repo_leaf}-{adjective}-{noun}` truncated to 124 bytes.
///
/// The caller is responsible for selecting the adjective+noun pair (typically from
/// [`ADJECTIVES`] and [`NOUNS`]) and iterating until no collision exists in the session
/// store.
pub fn shortname_for_main(repo_leaf: &str, adjective: &str, noun: &str) -> String {
    let leaf = sanitize_component(repo_leaf);
    let adj = sanitize_component(adjective);
    let noun_s = sanitize_component(noun);
    let combined = match (leaf.is_empty(), adj.is_empty(), noun_s.is_empty()) {
        (false, false, false) => format!("{leaf}-{adj}-{noun_s}"),
        (true, false, false) => format!("{adj}-{noun_s}"),
        (false, _, _) => leaf,
        _ => "session".to_owned(),
    };
    truncate_shortname(&combined)
}

/// Convert a stored shortname to a tmux session name by prepending `mux-`.
pub fn tmux_session_name(shortname: &str) -> String {
    format!("mux-{shortname}")
}

/// Suffix a shortname with a collision counter: `-2`, `-3`, etc.
///
/// The first shortname (no suffix) is attempt 1. This function returns suffixed
/// versions for attempts 2 and above.
///
/// Truncation is applied after appending the suffix to keep the result within 124 bytes.
pub fn shortname_with_suffix(base: &str, attempt: u32) -> String {
    if attempt <= 1 {
        return base.to_owned();
    }
    let suffix = format!("-{attempt}");
    let available = MAX_SHORTNAME_BYTES.saturating_sub(suffix.len());
    let trimmed_base = if base.len() > available {
        &base[..available]
    } else {
        base
    };
    let trimmed_base = trimmed_base.trim_end_matches('-');
    format!("{trimmed_base}{suffix}")
}

/// Adjectives used for main-branch shortname generation.
///
/// Combined with [`NOUNS`], these produce human-memorable `adjective-noun` suffixes.
pub const ADJECTIVES: &[&str] = &[
    "brave", "calm", "dark", "eager", "fair", "glad", "hazy", "idle", "jolly", "keen", "lazy",
    "mild", "neat", "odd", "pale", "quick", "rare", "soft", "tame", "vast",
];

/// Nouns used for main-branch shortname generation.
pub const NOUNS: &[&str] = &[
    "ant", "bat", "cat", "dove", "eel", "fox", "gnu", "hen", "ibis", "jay", "kite", "lark", "mole",
    "newt", "owl", "pug", "quail", "rat", "slug", "toad",
];

#[cfg(test)]
mod tests {
    use super::*;

    // sanitize_component

    #[test]
    fn sanitize_component_alphanumeric_passthrough() {
        assert_eq!(sanitize_component("abc123"), "abc123");
    }

    #[test]
    fn sanitize_component_lowercases() {
        assert_eq!(sanitize_component("MyBranch"), "mybranch");
    }

    #[test]
    fn sanitize_component_replaces_non_alnum_with_hyphen() {
        assert_eq!(
            sanitize_component("feature/my-feature"),
            "feature-my-feature"
        );
        assert_eq!(sanitize_component("my_branch"), "my-branch");
        assert_eq!(sanitize_component("branch.name"), "branch-name");
    }

    #[test]
    fn sanitize_component_collapses_runs_of_hyphens() {
        assert_eq!(sanitize_component("my--branch"), "my-branch");
        assert_eq!(sanitize_component("a///b"), "a-b");
    }

    #[test]
    fn sanitize_component_strips_leading_trailing_hyphens() {
        assert_eq!(sanitize_component("-branch-"), "branch");
        assert_eq!(sanitize_component("/branch/"), "branch");
    }

    #[test]
    fn sanitize_component_empty_becomes_empty() {
        assert_eq!(sanitize_component(""), "");
        assert_eq!(sanitize_component("---"), "");
        assert_eq!(sanitize_component("///"), "");
    }

    // truncate_shortname

    #[test]
    fn truncate_short_string_unchanged() {
        let s = "mux-feature";
        assert_eq!(truncate_shortname(s), s);
    }

    #[test]
    fn truncate_at_hyphen_boundary() {
        // Build a string that is 125 bytes: "aaa...a-bbb...b", truncate should find last hyphen before byte 124
        let base = "a".repeat(60) + "-" + &"b".repeat(64);
        assert_eq!(base.len(), 125);
        let result = truncate_shortname(&base);
        assert!(result.len() <= MAX_SHORTNAME_BYTES);
        assert!(!result.ends_with('-'));
        assert_eq!(result, "a".repeat(60));
    }

    #[test]
    fn truncate_hard_at_124_when_no_hyphen() {
        let s = "a".repeat(200);
        let result = truncate_shortname(&s);
        assert_eq!(result.len(), MAX_SHORTNAME_BYTES);
        assert_eq!(result, "a".repeat(MAX_SHORTNAME_BYTES));
    }

    #[test]
    fn truncate_exactly_124_unchanged() {
        let s = "a".repeat(MAX_SHORTNAME_BYTES);
        assert_eq!(truncate_shortname(&s).len(), MAX_SHORTNAME_BYTES);
    }

    // shortname_for_branch

    #[test]
    fn shortname_for_branch_basic() {
        assert_eq!(shortname_for_branch("mux", "main"), "mux-main");
        assert_eq!(
            shortname_for_branch("mux", "feature/my-feature"),
            "mux-feature-my-feature"
        );
    }

    #[test]
    fn shortname_for_branch_deterministic() {
        let a = shortname_for_branch("mux", "feature/auth");
        let b = shortname_for_branch("mux", "feature/auth");
        assert_eq!(a, b);
    }

    #[test]
    fn shortname_for_branch_sanitizes_components() {
        // underscores → hyphens
        assert_eq!(
            shortname_for_branch("my_repo", "my_branch"),
            "my-repo-my-branch"
        );
    }

    #[test]
    fn shortname_for_branch_truncates_to_124() {
        let repo = "r".repeat(100);
        let branch = "b".repeat(100);
        let result = shortname_for_branch(&repo, &branch);
        assert!(result.len() <= MAX_SHORTNAME_BYTES);
    }

    // shortname_for_main

    #[test]
    fn shortname_for_main_basic() {
        assert_eq!(
            shortname_for_main("mux", "happy", "panda"),
            "mux-happy-panda"
        );
    }

    #[test]
    fn shortname_for_main_deterministic() {
        assert_eq!(
            shortname_for_main("myrepo", "brave", "owl"),
            shortname_for_main("myrepo", "brave", "owl")
        );
    }

    #[test]
    fn shortname_for_main_no_main_master_suffix() {
        // main/master do NOT appear in the generated name — only repo leaf and adj-noun
        let result = shortname_for_main("mux", "calm", "fox");
        assert!(!result.contains("main"));
        assert!(!result.contains("master"));
        assert_eq!(result, "mux-calm-fox");
    }

    // tmux_session_name

    #[test]
    fn tmux_session_name_prepends_mux() {
        assert_eq!(tmux_session_name("mux-happy-panda"), "mux-mux-happy-panda");
        assert_eq!(tmux_session_name("feature-auth"), "mux-feature-auth");
    }

    // shortname_with_suffix

    #[test]
    fn shortname_with_suffix_attempt_1_unchanged() {
        assert_eq!(shortname_with_suffix("my-session", 1), "my-session");
    }

    #[test]
    fn shortname_with_suffix_appends_counter() {
        assert_eq!(shortname_with_suffix("my-session", 2), "my-session-2");
        assert_eq!(shortname_with_suffix("my-session", 10), "my-session-10");
    }

    #[test]
    fn shortname_with_suffix_stays_within_124() {
        let base = "a".repeat(MAX_SHORTNAME_BYTES);
        let result = shortname_with_suffix(&base, 2);
        assert!(result.len() <= MAX_SHORTNAME_BYTES);
        assert!(result.ends_with("-2"));
    }

    // word lists

    #[test]
    fn word_lists_non_empty() {
        assert!(!ADJECTIVES.is_empty());
        assert!(!NOUNS.is_empty());
    }

    #[test]
    fn word_list_entries_are_valid_components() {
        for &adj in ADJECTIVES {
            assert_eq!(
                sanitize_component(adj),
                adj,
                "adjective {adj:?} is not clean"
            );
        }
        for &noun in NOUNS {
            assert_eq!(sanitize_component(noun), noun, "noun {noun:?} is not clean");
        }
    }
}
