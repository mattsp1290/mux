// MUX_HOME resolution per docs/01 §Global behaviour

use std::path::PathBuf;

use mux_core::error::MuxError;

/// Resolve the mux state directory.
///
/// Priority order:
/// 1. `override_path` if Some (from --mux-home CLI flag)
/// 2. `MUX_HOME` environment variable
/// 3. `~/.mux` (home directory from HOME env var)
///
/// Returns MuxError::HomeDirNotFound if home cannot be determined.
pub fn resolve_mux_home(override_path: Option<PathBuf>) -> Result<PathBuf, MuxError> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    if let Ok(val) = std::env::var("MUX_HOME") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }
    // Fall back to ~/.mux
    let home = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(MuxError::HomeDirNotFound)?;
    Ok(home.join(".mux"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_path_takes_priority() {
        let path = PathBuf::from("/custom/mux/home");
        let result = resolve_mux_home(Some(path.clone())).unwrap();
        assert_eq!(result, path);
    }

    #[test]
    fn mux_home_env_var_used() {
        // Save and restore to minimise cross-test pollution (tests run in-process).
        let prev = std::env::var("MUX_HOME").ok();
        std::env::set_var("MUX_HOME", "/env/mux/home");

        let result = resolve_mux_home(None).unwrap();
        assert_eq!(result, PathBuf::from("/env/mux/home"));

        // Restore
        match prev {
            Some(v) => std::env::set_var("MUX_HOME", v),
            None => std::env::remove_var("MUX_HOME"),
        }
    }

    #[test]
    fn defaults_to_dot_mux() {
        let prev_mux = std::env::var("MUX_HOME").ok();
        let prev_home = std::env::var("HOME").ok();

        std::env::remove_var("MUX_HOME");
        std::env::set_var("HOME", "/home/testuser");

        let result = resolve_mux_home(None).unwrap();
        assert_eq!(result, PathBuf::from("/home/testuser/.mux"));

        // Restore
        match prev_mux {
            Some(v) => std::env::set_var("MUX_HOME", v),
            None => std::env::remove_var("MUX_HOME"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn empty_mux_home_falls_back() {
        let prev_mux = std::env::var("MUX_HOME").ok();
        let prev_home = std::env::var("HOME").ok();

        std::env::set_var("MUX_HOME", "");
        std::env::set_var("HOME", "/home/fallback");

        let result = resolve_mux_home(None).unwrap();
        assert_eq!(result, PathBuf::from("/home/fallback/.mux"));

        // Restore
        match prev_mux {
            Some(v) => std::env::set_var("MUX_HOME", v),
            None => std::env::remove_var("MUX_HOME"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
