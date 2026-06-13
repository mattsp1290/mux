//! Workdir construction and safety classification.
//!
//! A session workdir is considered mux-created (and therefore removable) only if:
//! - Path matches `$MUX_HOME/<uuid>/<repo-leaf>`
//! - No symlinks exist in the path components.
//!
//! Workdirs not matching this pattern (imported sessions) must never be removed.

use std::path::{Path, PathBuf};

/// Constructs the canonical workdir path: `mux_home/<uuid>/<repo_leaf>`.
///
/// Pure path construction — no filesystem access.
pub fn build_workdir(mux_home: &Path, uuid: &str, repo_leaf: &str) -> PathBuf {
    mux_home.join(uuid).join(repo_leaf)
}

/// Constructs the workdir parent path: `mux_home/<uuid>`.
///
/// This is used in the CreateSession RPC request as `workdir_parent`.
/// Pure path construction — no filesystem access.
pub fn build_workdir_parent(mux_home: &Path, uuid: &str) -> PathBuf {
    mux_home.join(uuid)
}

/// Classification of a workdir path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkdirClassification {
    /// Path matches `$MUX_HOME/<uuid>/<repo-leaf>` pattern and contains no symlinks.
    /// This workdir may be removed by mux.
    MuxCreated,
    /// Path does not match the mux pattern or contains symlinks.
    /// This workdir must NOT be removed.
    Imported,
}

/// Classifies a workdir path based on its structural pattern.
///
/// Does NOT access the filesystem — purely structural path analysis.
/// The symlink check is the caller's responsibility for live paths.
///
/// Returns `MuxCreated` if:
/// - `workdir` starts with `mux_home`
/// - Exactly 2 components follow `mux_home`
/// - First component looks like a UUID (36 chars, hyphens at 8,13,18,23, rest are hex)
/// - Second component is non-empty (the repo leaf)
///
/// Returns `Imported` otherwise.
pub fn classify_workdir(workdir: &Path, mux_home: &Path) -> WorkdirClassification {
    // Strip the mux_home prefix — if it doesn't start with mux_home, it's Imported.
    let relative = match workdir.strip_prefix(mux_home) {
        Ok(rel) => rel,
        Err(_) => return WorkdirClassification::Imported,
    };

    let mut components = relative.components();

    // First component: UUID
    let uuid_component = match components.next() {
        Some(c) => c,
        None => return WorkdirClassification::Imported,
    };
    let uuid_str = match uuid_component.as_os_str().to_str() {
        Some(s) => s,
        None => return WorkdirClassification::Imported,
    };
    if !looks_like_uuid(uuid_str) {
        return WorkdirClassification::Imported;
    }

    // Second component: repo leaf
    let leaf_component = match components.next() {
        Some(c) => c,
        None => return WorkdirClassification::Imported,
    };
    let leaf_str = match leaf_component.as_os_str().to_str() {
        Some(s) => s,
        None => return WorkdirClassification::Imported,
    };
    if leaf_str.is_empty() {
        return WorkdirClassification::Imported;
    }

    // No additional components allowed
    if components.next().is_some() {
        return WorkdirClassification::Imported;
    }

    WorkdirClassification::MuxCreated
}

/// Returns `true` only if the workdir is safe to remove.
///
/// Safe to remove means:
/// 1. `classify_workdir` returns `MuxCreated`
/// 2. No symlink exists in any path component up to and including the leaf
///    (checked via `std::fs::symlink_metadata`)
pub fn is_safe_to_remove(workdir: &Path, mux_home: &Path) -> bool {
    if classify_workdir(workdir, mux_home) != WorkdirClassification::MuxCreated {
        return false;
    }

    // Walk each prefix of the path and check for symlinks.
    // We start from the root and build up, checking each component.
    let mut current = PathBuf::new();
    for component in workdir.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return false;
                }
            }
            Err(_) => {
                // If we can't stat a component, assume it's not safe.
                return false;
            }
        }
    }

    true
}

/// Returns `true` if `s` looks like a valid UUID string.
///
/// Checks:
/// - Length is exactly 36
/// - Hyphens at positions 8, 13, 18, 23
/// - All other characters are ASCII hex digits (0–9, a–f, A–F)
pub(crate) fn looks_like_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    // Check hyphens at the expected positions
    if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
        return false;
    }
    // All other positions must be hex digits
    for (i, &b) in bytes.iter().enumerate() {
        if i == 8 || i == 13 || i == 18 || i == 23 {
            continue;
        }
        if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // A valid UUID for use in tests
    const VALID_UUID: &str = "6bac8714-c91a-45ec-84b8-7f384b9988f7";

    #[test]
    fn build_workdir_constructs_correct_path() {
        let result = build_workdir(Path::new("/home/user/.mux"), "abc-uuid", "myrepo");
        assert_eq!(result, Path::new("/home/user/.mux/abc-uuid/myrepo"));
    }

    #[test]
    fn build_workdir_parent_constructs_correct_path() {
        let result = build_workdir_parent(Path::new("/home/user/.mux"), "abc-uuid");
        assert_eq!(result, Path::new("/home/user/.mux/abc-uuid"));
    }

    #[test]
    fn classify_mux_created_path() {
        let mux_home = Path::new("/home/user/.mux");
        let workdir = mux_home.join(VALID_UUID).join("myrepo");
        assert_eq!(
            classify_workdir(&workdir, mux_home),
            WorkdirClassification::MuxCreated
        );
    }

    #[test]
    fn classify_non_mux_path() {
        let mux_home = Path::new("/home/user/.mux");
        let workdir = Path::new("/tmp/something");
        assert_eq!(
            classify_workdir(workdir, mux_home),
            WorkdirClassification::Imported
        );
    }

    #[test]
    fn classify_too_short() {
        // Only uuid component, missing repo leaf
        let mux_home = Path::new("/home/user/.mux");
        let workdir = mux_home.join(VALID_UUID);
        assert_eq!(
            classify_workdir(&workdir, mux_home),
            WorkdirClassification::Imported
        );
    }

    #[test]
    fn classify_too_deep() {
        // uuid + leaf + extra component
        let mux_home = Path::new("/home/user/.mux");
        let workdir = mux_home.join(VALID_UUID).join("myrepo").join("extra");
        assert_eq!(
            classify_workdir(&workdir, mux_home),
            WorkdirClassification::Imported
        );
    }

    #[test]
    fn classify_invalid_uuid_component() {
        // Too short to be a UUID
        let mux_home = Path::new("/home/user/.mux");
        let workdir = mux_home.join("not-a-uuid").join("myrepo");
        assert_eq!(
            classify_workdir(&workdir, mux_home),
            WorkdirClassification::Imported
        );
    }

    #[test]
    fn classify_validates_uuid_hyphens() {
        // Right length (36) but hyphens in wrong positions
        // Valid positions: 8, 13, 18, 23
        // This string has hyphens elsewhere
        let bad_uuid = "6bac871-4c91a-45ec84b8-7f384b9988f7x";
        assert_eq!(bad_uuid.len(), 36);
        let mux_home = Path::new("/home/user/.mux");
        let workdir = mux_home.join(bad_uuid).join("myrepo");
        assert_eq!(
            classify_workdir(&workdir, mux_home),
            WorkdirClassification::Imported
        );
    }

    #[test]
    fn is_safe_to_remove_real_dir() {
        // Create a temp dir, then build a mux-style path inside it.
        let tmp = tempfile::TempDir::new().unwrap();
        let mux_home = tmp.path();
        let uuid_dir = mux_home.join(VALID_UUID);
        let workdir = uuid_dir.join("myrepo");
        std::fs::create_dir_all(&workdir).unwrap();

        assert!(is_safe_to_remove(&workdir, mux_home));
    }

    #[test]
    fn is_safe_to_remove_symlink_in_path() {
        // Create a temp dir structure where the uuid dir is a symlink.
        let tmp = tempfile::TempDir::new().unwrap();
        let mux_home = tmp.path();

        // Create a real target directory
        let real_target = tmp.path().join("real_target");
        std::fs::create_dir_all(&real_target).unwrap();

        // Create a symlink at the uuid position
        let uuid_link = mux_home.join(VALID_UUID);
        std::os::unix::fs::symlink(&real_target, &uuid_link).unwrap();

        // Create the repo leaf inside the symlinked dir
        let workdir = uuid_link.join("myrepo");
        std::fs::create_dir_all(&workdir).unwrap();

        // Should be false because there's a symlink in the path
        assert!(!is_safe_to_remove(&workdir, mux_home));
    }

    #[test]
    fn is_safe_to_remove_imported_path() {
        // A path that doesn't match the mux pattern
        let tmp = tempfile::TempDir::new().unwrap();
        let mux_home = tmp.path();
        let imported = Path::new("/tmp/some-imported-project");
        assert!(!is_safe_to_remove(imported, mux_home));
    }
}
