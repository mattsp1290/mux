use thiserror::Error;

#[non_exhaustive]
#[derive(Debug, Error)]
pub enum MuxError {
    #[error("invalid host alias: {0}")]
    InvalidHostAlias(String),

    /// Carries the raw input string so diagnostics can show what the user typed.
    #[error("invalid port: {0}")]
    InvalidPort(String),

    /// Raised when parsing `user@addr` endpoint strings.
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),

    /// Raised when loading session status from SQLite — implemented in mux-7sa.
    #[error("invalid session status: {0}")]
    InvalidSessionStatus(String),

    /// Raised when resolving the mux state directory — implemented in mux-init.
    #[error("home directory not found")]
    HomeDirNotFound,
}
