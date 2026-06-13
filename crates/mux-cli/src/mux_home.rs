use std::path::PathBuf;

use mux_core::error::MuxError;

/// Resolve the mux state directory.
///
/// Priority order:
/// 1. `override_path` if `Some` (from `--mux-home` CLI flag)
/// 2. `MUX_HOME` environment variable (if set and non-empty)
/// 3. `~/.mux` (home directory from `HOME` env var — Unix-only; mux targets Unix/tmux)
///
/// Returns `MuxError::HomeDirNotFound` if home cannot be determined.
pub fn resolve_mux_home(override_path: Option<PathBuf>) -> Result<PathBuf, MuxError> {
    let mux_home_env = std::env::var("MUX_HOME").ok();
    let home_env = std::env::var("HOME").ok();
    resolve_mux_home_from(override_path, mux_home_env.as_deref(), home_env.as_deref())
}

/// Pure resolution logic — accepts env values as parameters to allow testing
/// without mutating global process state.
fn resolve_mux_home_from(
    override_path: Option<PathBuf>,
    mux_home_env: Option<&str>,
    home_env: Option<&str>,
) -> Result<PathBuf, MuxError> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    if let Some(val) = mux_home_env {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }
    let home = home_env
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .ok_or(MuxError::HomeDirNotFound)?;
    Ok(home.join(".mux"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(
        override_path: Option<PathBuf>,
        mux_home_env: Option<&str>,
        home_env: Option<&str>,
    ) -> Result<PathBuf, MuxError> {
        resolve_mux_home_from(override_path, mux_home_env, home_env)
    }

    #[test]
    fn override_path_takes_priority() {
        let path = PathBuf::from("/custom/mux/home");
        let result = r(Some(path.clone()), Some("/env/mux"), Some("/home/user")).unwrap();
        assert_eq!(result, path);
    }

    #[test]
    fn mux_home_env_used_when_no_override() {
        let result = r(None, Some("/env/mux/home"), Some("/home/user")).unwrap();
        assert_eq!(result, PathBuf::from("/env/mux/home"));
    }

    #[test]
    fn defaults_to_dot_mux() {
        let result = r(None, None, Some("/home/testuser")).unwrap();
        assert_eq!(result, PathBuf::from("/home/testuser/.mux"));
    }

    #[test]
    fn empty_mux_home_falls_back() {
        let result = r(None, Some(""), Some("/home/fallback")).unwrap();
        assert_eq!(result, PathBuf::from("/home/fallback/.mux"));
    }

    #[test]
    fn home_dir_not_found_when_home_missing() {
        let err = r(None, None, None).unwrap_err();
        assert!(
            matches!(err, MuxError::HomeDirNotFound),
            "expected HomeDirNotFound, got {err:?}"
        );
    }

    #[test]
    fn home_dir_not_found_when_home_empty() {
        let err = r(None, None, Some("")).unwrap_err();
        assert!(matches!(err, MuxError::HomeDirNotFound));
    }
}
