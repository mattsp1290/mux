//! Integration tests for naming and workdir behaviour documented in docs/02.
//!
//! These tests exercise the public API of `mux_core::shortname` and
//! `mux_core::workdir` from outside the crate, confirming that every documented
//! example and boundary value is reachable and correct.

use mux_core::{
    shortname::{
        sanitize_component, shortname_for_branch, shortname_for_main,
        shortname_with_suffix, tmux_session_name, truncate_shortname,
        main_namespace_size, ADJECTIVES, NOUNS, MAX_SHORTNAME_BYTES,
    },
    workdir::{
        build_workdir, build_workdir_parent, classify_workdir, is_safe_to_remove,
        WorkdirClassification,
    },
};
use std::path::Path;

const VALID_UUID: &str = "6bac8714-c91a-45ec-84b8-7f384b9988f7";

// ── Documented constants ──────────────────────────────────────────────────────

#[test]
fn max_shortname_bytes_is_124() {
    // docs/02: "Max length: 124 bytes (tmux session name limit with `mux-` prefix
    // overhead)."
    assert_eq!(MAX_SHORTNAME_BYTES, 124);
}

#[test]
fn main_namespace_is_400() {
    // docs/02: 20 adjectives × 20 nouns = 400 unique main-branch shortnames per repo.
    assert_eq!(ADJECTIVES.len(), 20);
    assert_eq!(NOUNS.len(), 20);
    assert_eq!(main_namespace_size(), 400);
}

// ── Shortname sanitisation (docs/02 "Shortname sanitisation") ────────────────

#[test]
fn sanitize_lowercases_ascii() {
    assert_eq!(sanitize_component("MyBranch"), "mybranch");
    assert_eq!(sanitize_component("FEATURE"), "feature");
}

#[test]
fn sanitize_replaces_non_alnum_with_hyphen() {
    // docs/02: "lowercase, replace non-alnum with `-`, collapse runs of `-`."
    assert_eq!(sanitize_component("feature/my-feature"), "feature-my-feature");
    assert_eq!(sanitize_component("my_branch"), "my-branch");
}

#[test]
fn sanitize_collapses_hyphen_runs() {
    assert_eq!(sanitize_component("a///b"), "a-b");
    assert_eq!(sanitize_component("my--branch"), "my-branch");
}

#[test]
fn sanitize_strips_leading_trailing_hyphens() {
    assert_eq!(sanitize_component("-branch-"), "branch");
}

#[test]
fn sanitize_all_special_chars_is_empty() {
    assert_eq!(sanitize_component("///"), "");
    assert_eq!(sanitize_component("---"), "");
}

// ── Non-main-branch shortname (docs/02 "Non-main-branch shortname") ──────────

#[test]
fn branch_shortname_docs_example() {
    // docs/02 example: "mux" repo, "feature/my-feature" branch → "mux-feature-my-feature"
    assert_eq!(
        shortname_for_branch("mux", "feature/my-feature"),
        "mux-feature-my-feature"
    );
}

#[test]
fn branch_shortname_is_deterministic() {
    // docs/02: "Deterministic: same repo+branch always produces the same shortname."
    let a = shortname_for_branch("mux", "feature/auth");
    let b = shortname_for_branch("mux", "feature/auth");
    assert_eq!(a, b);
}

#[test]
fn branch_shortname_sanitizes_both_components() {
    assert_eq!(shortname_for_branch("my_repo", "my_branch"), "my-repo-my-branch");
}

#[test]
fn branch_shortname_respects_max_length() {
    // docs/02: max 124 bytes.
    let result = shortname_for_branch(&"r".repeat(100), &"b".repeat(100));
    assert!(result.len() <= MAX_SHORTNAME_BYTES, "len={}", result.len());
}

#[test]
fn branch_shortname_truncates_at_hyphen_boundary() {
    // 62-char repo + hyphen + 62-char branch → 125 bytes before truncation.
    // truncation must cut at the joining hyphen, leaving the 62-char repo leaf.
    let repo = "r".repeat(62);
    let branch = "b".repeat(62);
    let result = shortname_for_branch(&repo, &branch);
    assert!(result.len() <= MAX_SHORTNAME_BYTES);
    assert!(!result.ends_with('-'), "trailing hyphen after truncation");
    assert_eq!(result, "r".repeat(62), "should cut at the joining hyphen");
}

// ── Main-branch shortname (docs/02 "Main-branch shortname") ──────────────────

#[test]
fn main_shortname_docs_example() {
    // docs/02 example: "mux-happy-panda"
    assert_eq!(shortname_for_main("mux", "happy", "panda"), "mux-happy-panda");
}

#[test]
fn main_shortname_no_main_or_master_in_result() {
    // docs/02: "The `main`/`master` suffix is NOT appended; a random suffix is used
    // instead."  shortname_for_main takes explicit adj+noun — neither word may be
    // "main" or "master" since those are not in the word lists.
    for &adj in ADJECTIVES {
        for &noun in NOUNS {
            let sn = shortname_for_main("repo", adj, noun);
            assert!(
                !sn.ends_with("-main") && !sn.ends_with("-master"),
                "shortname {sn:?} must not end with -main or -master"
            );
        }
    }
}

#[test]
fn main_shortname_word_lists_contain_no_main_or_master() {
    // Adjective and noun lists must not include "main" or "master" — docs/02 says the
    // main suffix is replaced by a random pair, so those words must not appear.
    for &adj in ADJECTIVES {
        assert_ne!(adj, "main");
        assert_ne!(adj, "master");
    }
    for &noun in NOUNS {
        assert_ne!(noun, "main");
        assert_ne!(noun, "master");
    }
}

#[test]
fn main_shortname_format_is_leaf_adj_noun() {
    // Format: {repo-leaf}-{adjective}-{noun}
    let sn = shortname_for_main("myrepo", "calm", "fox");
    assert_eq!(sn, "myrepo-calm-fox");
}

// ── Tmux prefix (docs/02 "Shortname sanitisation") ───────────────────────────

#[test]
fn tmux_session_name_prepends_mux_prefix() {
    // docs/02: "`mux-` prefix is prepended to every tmux session name to avoid
    // collisions with non-mux sessions."
    assert_eq!(tmux_session_name("mux-happy-panda"), "mux-mux-happy-panda");
    assert_eq!(tmux_session_name("feature-auth"), "mux-feature-auth");
}

#[test]
fn tmux_session_name_pipeline_from_main_shortname() {
    // Full pipeline: repo leaf + adj + noun → shortname → tmux name.
    // Uses ADJECTIVES[0]="brave" and NOUNS[0]="ant" — both real word-list entries.
    let shortname = shortname_for_main("mux", ADJECTIVES[0], NOUNS[0]);
    let tmux_name = tmux_session_name(&shortname);
    assert_eq!(tmux_name, "mux-mux-brave-ant");
    assert!(tmux_name.starts_with("mux-"));
}

#[test]
fn tmux_session_name_pipeline_from_branch_shortname() {
    // Full pipeline for a non-main branch
    let shortname = shortname_for_branch("mux", "feature/auth");
    let tmux_name = tmux_session_name(&shortname);
    assert_eq!(tmux_name, "mux-mux-feature-auth");
    assert!(tmux_name.starts_with("mux-"));
}

// ── Truncation (docs/02 "Shortname sanitisation") ────────────────────────────

#[test]
fn truncate_short_string_unchanged() {
    let s = "mux-happy-panda";
    assert_eq!(truncate_shortname(s), s);
}

#[test]
fn truncate_exactly_at_max_unchanged() {
    let s = "a".repeat(MAX_SHORTNAME_BYTES);
    assert_eq!(truncate_shortname(&s).len(), MAX_SHORTNAME_BYTES);
}

#[test]
fn truncate_cuts_at_hyphen_boundary() {
    // docs/02: "truncate at a hyphen boundary where possible."
    // 60 a's + "-" + 64 b's = 125 bytes — one over the limit.
    let base = "a".repeat(60) + "-" + &"b".repeat(64);
    let result = truncate_shortname(&base);
    assert!(result.len() <= MAX_SHORTNAME_BYTES);
    assert!(!result.ends_with('-'), "must not leave a trailing hyphen");
    assert_eq!(result, "a".repeat(60), "should cut cleanly at the hyphen");
}

#[test]
fn truncate_hard_truncates_when_no_hyphen() {
    // docs/02: "hard-truncate at 124 if no boundary is available."
    let s = "a".repeat(200);
    let result = truncate_shortname(&s);
    assert_eq!(result.len(), MAX_SHORTNAME_BYTES);
}

#[test]
fn truncate_does_not_split_multibyte_char() {
    // 'é' is 2 bytes; ensure the whole multi-byte char is dropped, not split.
    let s = "a".repeat(123) + "é"; // 125 bytes total
    let result = truncate_shortname(&s);
    assert!(result.len() <= MAX_SHORTNAME_BYTES);
    assert_eq!(result, "a".repeat(123), "multi-byte char must be dropped whole");
}

// ── Collision-handling suffix (docs/02 "Non-main-branch shortname") ───────────

#[test]
fn suffix_attempt_1_returns_base() {
    // docs/02: collision "resolved by appending `-2`, `-3`, etc."
    // Attempt 1 = no suffix (the unsuffixed base).
    assert_eq!(shortname_with_suffix("my-session", 1), "my-session");
}

#[test]
fn suffix_attempt_2_appends_numeric() {
    assert_eq!(shortname_with_suffix("my-session", 2), "my-session-2");
}

#[test]
fn suffix_attempt_10_appends_10() {
    assert_eq!(shortname_with_suffix("my-session", 10), "my-session-10");
}

#[test]
fn suffix_stays_within_max_when_base_is_full_length() {
    let base = "a".repeat(MAX_SHORTNAME_BYTES);
    let result = shortname_with_suffix(&base, 2);
    assert_eq!(result.len(), MAX_SHORTNAME_BYTES, "result should use the full 124 bytes");
    assert!(result.ends_with("-2"), "suffix must be present");
}

// ── Workdir path construction ────────────────────────────────────────────────

#[test]
fn workdir_path_matches_docs_pattern() {
    // docs/02: "Path matches $MUX_HOME/<uuid>/<repo-leaf>"
    let mux_home = Path::new("/home/user/.mux");
    let wd = build_workdir(mux_home, VALID_UUID, "mux");
    assert_eq!(wd, Path::new("/home/user/.mux/6bac8714-c91a-45ec-84b8-7f384b9988f7/mux"));
}

#[test]
fn workdir_parent_is_uuid_dir() {
    let mux_home = Path::new("/home/user/.mux");
    let parent = build_workdir_parent(mux_home, VALID_UUID);
    assert_eq!(
        parent,
        Path::new("/home/user/.mux/6bac8714-c91a-45ec-84b8-7f384b9988f7")
    );
}

#[test]
fn build_workdir_then_parent_consistency() {
    // build_workdir_parent must equal the parent of build_workdir.
    let mux_home = Path::new("/data/.mux");
    let wd = build_workdir(mux_home, VALID_UUID, "myrepo");
    let parent = build_workdir_parent(mux_home, VALID_UUID);
    assert_eq!(wd.parent().unwrap(), parent);
}

// ── Workdir safety classification (docs/02 "Workdir safety") ─────────────────

#[test]
fn workdir_mux_created_for_canonical_path() {
    // docs/02: MuxCreated iff path matches $MUX_HOME/<uuid>/<repo-leaf> with no symlinks.
    let mux_home = Path::new("/home/user/.mux");
    let wd = build_workdir(mux_home, VALID_UUID, "mux");
    assert_eq!(classify_workdir(&wd, mux_home), WorkdirClassification::MuxCreated);
}

#[test]
fn workdir_imported_for_path_outside_mux_home() {
    // docs/02: "Workdirs not matching this pattern (imported sessions) must never be
    // removed."
    let mux_home = Path::new("/home/user/.mux");
    assert_eq!(
        classify_workdir(Path::new("/tmp/myproject"), mux_home),
        WorkdirClassification::Imported
    );
    assert_eq!(
        classify_workdir(Path::new("/home/user/projects/mux"), mux_home),
        WorkdirClassification::Imported
    );
}

#[test]
fn workdir_imported_when_only_uuid_depth() {
    // $MUX_HOME/<uuid> alone (no leaf) is not a valid workdir.
    let mux_home = Path::new("/home/user/.mux");
    let uuid_dir = mux_home.join(VALID_UUID);
    assert_eq!(
        classify_workdir(&uuid_dir, mux_home),
        WorkdirClassification::Imported
    );
}

#[test]
fn workdir_imported_when_too_deep() {
    // $MUX_HOME/<uuid>/<leaf>/extra must be Imported — too many components.
    let mux_home = Path::new("/home/user/.mux");
    let deep = mux_home.join(VALID_UUID).join("leaf").join("extra");
    assert_eq!(classify_workdir(&deep, mux_home), WorkdirClassification::Imported);
}

#[test]
fn workdir_dotdot_in_leaf_position_is_imported() {
    // Security: $MUX_HOME/<uuid>/.. would resolve to $MUX_HOME, escaping the intended
    // subtree. Must be classified Imported and never removed.
    let mux_home = Path::new("/home/user/.mux");
    let traversal = mux_home.join(VALID_UUID).join("..");
    assert_eq!(
        classify_workdir(&traversal, mux_home),
        WorkdirClassification::Imported,
        "$MUX_HOME/<uuid>/.. must not be MuxCreated"
    );
}

#[test]
fn workdir_dotdot_in_uuid_position_is_imported() {
    // $MUX_HOME/../<something> must be Imported (non-UUID first component).
    let mux_home = Path::new("/home/user/.mux");
    let traversal = mux_home.join("..").join("leaf");
    assert_eq!(classify_workdir(&traversal, mux_home), WorkdirClassification::Imported);
}

#[test]
fn workdir_invalid_uuid_format_is_imported() {
    let mux_home = Path::new("/home/user/.mux");
    let bad_uuid_dir = mux_home.join("not-a-valid-uuid").join("leaf");
    assert_eq!(classify_workdir(&bad_uuid_dir, mux_home), WorkdirClassification::Imported);
}

// ── is_safe_to_remove filesystem tests ───────────────────────────────────────

#[test]
fn is_safe_to_remove_real_canonical_dir() {
    // docs/02: MuxCreated + no symlinks → safe to remove.
    let tmp = tempfile::TempDir::new().unwrap();
    let mux_home = tmp.path();
    let wd = build_workdir(mux_home, VALID_UUID, "mux");
    std::fs::create_dir_all(&wd).unwrap();
    assert!(is_safe_to_remove(&wd, mux_home));
}

#[test]
fn is_safe_to_remove_false_for_imported_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mux_home = tmp.path();
    let imported = Path::new("/tmp/some-project");
    assert!(!is_safe_to_remove(imported, mux_home));
}

#[test]
fn is_safe_to_remove_false_when_uuid_dir_is_symlink() {
    // docs/02: "No symlinks in the path."
    let tmp = tempfile::TempDir::new().unwrap();
    let mux_home = tmp.path();

    let real_target = tmp.path().join("real_uuid_dir");
    std::fs::create_dir_all(&real_target).unwrap();

    let uuid_link = mux_home.join(VALID_UUID);
    std::os::unix::fs::symlink(&real_target, &uuid_link).unwrap();

    let wd = uuid_link.join("mux");
    std::fs::create_dir_all(&wd).unwrap();

    assert!(!is_safe_to_remove(&wd, mux_home));
}

#[test]
fn is_safe_to_remove_false_when_leaf_is_symlink() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mux_home = tmp.path();

    let uuid_dir = mux_home.join(VALID_UUID);
    std::fs::create_dir_all(&uuid_dir).unwrap();

    let real_target = tmp.path().join("some_other_dir");
    std::fs::create_dir_all(&real_target).unwrap();

    let leaf_link = uuid_dir.join("mux");
    std::os::unix::fs::symlink(&real_target, &leaf_link).unwrap();

    assert!(!is_safe_to_remove(&leaf_link, mux_home));
}

// ── Additional coverage for reviewed gaps ─────────────────────────────────────

#[test]
fn branch_shortname_empty_components_fall_back_to_session() {
    // docs/02 "Non-main-branch shortname": when both leaf and branch sanitize to empty,
    // the fallback shortname "session" is returned.
    assert_eq!(shortname_for_branch("///", "---"), "session");
    // Only leaf sanitizes to empty — result is the sanitized branch.
    assert_eq!(shortname_for_branch("///", "feature/x"), "feature-x");
    // Only branch sanitizes to empty — result is the sanitized leaf.
    assert_eq!(shortname_for_branch("myrepo", "///"), "myrepo");
}

#[test]
fn workdir_dot_in_leaf_position_is_imported() {
    // Symmetric to the ".." test: "." is a current-dir Component, not Normal, so
    // $MUX_HOME/<uuid>/. must also be Imported.
    let mux_home = Path::new("/home/user/.mux");
    let traversal = mux_home.join(VALID_UUID).join(".");
    assert_eq!(
        classify_workdir(&traversal, mux_home),
        WorkdirClassification::Imported,
        "$MUX_HOME/<uuid>/. must not be MuxCreated"
    );
}

#[test]
#[should_panic(expected = "attempt is 1-based, got 0")]
fn suffix_attempt_0_panics() {
    // shortname_with_suffix is 1-based; 0 is a programming error and panics in debug.
    shortname_with_suffix("my-session", 0);
}
