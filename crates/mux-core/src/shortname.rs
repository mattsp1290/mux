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

/// Fallback shortname used when both repo-leaf and branch sanitize to empty.
const FALLBACK_SHORTNAME: &str = "session";

/// Largest byte index `<= max` that is a valid UTF-8 char boundary in `s`.
///
/// Used before fixed-byte slices to avoid panicking on multibyte input.
fn floor_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Truncate `s` to at most `max` bytes, preferring a hyphen boundary.
///
/// Internal helper shared by [`truncate_shortname`] and [`shortname_with_suffix`].
fn truncate_to(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let cut = floor_boundary(s, max);
    let prefix = &s[..cut];
    match prefix.rfind('-') {
        Some(pos) if pos > 0 => prefix[..pos].to_owned(),
        _ => prefix.to_owned(),
    }
}

/// Sanitize a component (repo leaf or branch name) for use in a shortname.
///
/// Rules: lowercase, replace any non-ASCII-alphanumeric character with `-`, collapse
/// consecutive hyphens, strip leading and trailing hyphens.
///
/// Post-condition: output contains only `[a-z0-9-]`, has no leading/trailing hyphen,
/// and has no consecutive hyphens.
pub fn sanitize_component(s: &str) -> String {
    let mut out = String::new();
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
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
/// 124 bytes, hard-truncates at 124 bytes. Safe for arbitrary UTF-8 input.
///
/// Note: this function only enforces length — it does not strip leading/trailing hyphens
/// or validate character content. Call [`sanitize_component`] first for clean output.
pub fn truncate_shortname(s: &str) -> String {
    truncate_to(s, MAX_SHORTNAME_BYTES)
}

/// Generate a deterministic shortname for a non-main branch.
///
/// Format: `{sanitized_repo_leaf}-{sanitized_branch}` truncated to 124 bytes.
///
/// Same `repo_leaf` + `branch` input always produces the same output (stable).
/// Collision with existing shortnames is resolved by the caller via
/// [`shortname_with_suffix`] (append `-2`, `-3`, etc.).
pub fn shortname_for_branch(repo_leaf: &str, branch: &str) -> String {
    let leaf = sanitize_component(repo_leaf);
    let branch_s = sanitize_component(branch);
    if leaf.is_empty() && branch_s.is_empty() {
        return FALLBACK_SHORTNAME.to_owned();
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
/// The caller is responsible for selecting the adjective+noun pair (typically by
/// iterating all pairs from [`ADJECTIVES`] × [`NOUNS`], in random order, until one
/// produces a shortname that is not already present in the session store).
/// [`main_namespace_size`] gives the total number of available pairs.
pub fn shortname_for_main(repo_leaf: &str, adjective: &str, noun: &str) -> String {
    let leaf = sanitize_component(repo_leaf);
    let adj = sanitize_component(adjective);
    let noun_s = sanitize_component(noun);
    let combined = match (leaf.is_empty(), adj.is_empty(), noun_s.is_empty()) {
        (false, false, false) => format!("{leaf}-{adj}-{noun_s}"),
        (true, false, false) => format!("{adj}-{noun_s}"),
        (false, _, _) => leaf,
        _ => FALLBACK_SHORTNAME.to_owned(),
    };
    truncate_shortname(&combined)
}

/// Convert a stored shortname to a tmux session name by prepending `mux-`.
pub fn tmux_session_name(shortname: &str) -> String {
    format!("mux-{shortname}")
}

/// Suffix a shortname with a collision counter: `-2`, `-3`, etc.
///
/// `attempt` is **1-based**: attempt 1 returns the base unchanged (no suffix);
/// attempt 2 appends `-2`, attempt 3 appends `-3`, and so on.
/// Passing 0 is a programming error and panics in debug builds.
///
/// The result is truncated to [`MAX_SHORTNAME_BYTES`] at a hyphen boundary where
/// possible — the same rule as [`truncate_shortname`] — so the output always fits in
/// the byte budget.
///
/// Note: two very long bases that share a common prefix may truncate to the same
/// suffixed name. The caller is responsible for verifying uniqueness in the session
/// store after applying the suffix.
pub fn shortname_with_suffix(base: &str, attempt: u32) -> String {
    debug_assert!(
        attempt >= 1,
        "shortname_with_suffix: attempt is 1-based, got 0"
    );
    if attempt <= 1 {
        return base.to_owned();
    }
    let suffix = format!("-{attempt}");
    let available = MAX_SHORTNAME_BYTES.saturating_sub(suffix.len());
    let trimmed = truncate_to(base, available);
    let trimmed = trimmed.trim_end_matches('-');
    format!("{trimmed}{suffix}")
}

/// Total number of distinct adjective-noun pairs in the word lists.
///
/// A caller iterating pairs to find a unique main-branch shortname can use this to
/// detect exhaustion of the namespace (all `ADJECTIVES.len() × NOUNS.len()` pairs
/// have been tried).
pub fn main_namespace_size() -> usize {
    ADJECTIVES.len() * NOUNS.len()
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
        // 125 bytes: "aaa...a-bbb...b"; truncate should cut at the hyphen at byte 60
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

    #[test]
    fn truncate_multibyte_does_not_panic() {
        // 'é' is 2 bytes; 123×'a' + 'é' = 125 bytes, byte 124 is mid-'é'
        let s = "a".repeat(123) + "é";
        assert_eq!(s.len(), 125);
        let result = truncate_shortname(&s);
        assert!(result.len() <= MAX_SHORTNAME_BYTES);
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

    #[test]
    fn shortname_for_branch_empty_leaf_uses_branch() {
        assert_eq!(shortname_for_branch("///", "feature/x"), "feature-x");
    }

    #[test]
    fn shortname_for_branch_empty_branch_uses_leaf() {
        assert_eq!(shortname_for_branch("myrepo", "///"), "myrepo");
    }

    #[test]
    fn shortname_for_branch_both_empty_is_fallback() {
        assert_eq!(shortname_for_branch("///", "---"), FALLBACK_SHORTNAME);
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
    #[should_panic(expected = "attempt is 1-based, got 0")]
    fn shortname_with_suffix_attempt_0_panics_in_debug() {
        shortname_with_suffix("my-session", 0);
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

    #[test]
    fn shortname_with_suffix_multibyte_does_not_panic() {
        // 121×'a' + 'é' = 123 bytes; available for "-2" is 122, which splits 'é'
        let base = "a".repeat(121) + "é";
        assert_eq!(base.len(), 123);
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

    #[test]
    fn main_namespace_size_is_product_of_lists() {
        assert_eq!(
            main_namespace_size(),
            ADJECTIVES.len() * NOUNS.len(),
            "namespace size should be 20×20"
        );
        assert_eq!(main_namespace_size(), 400);
    }
}
