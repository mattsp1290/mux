use thiserror::Error;

#[non_exhaustive]
#[derive(Debug, Error)]
pub enum MuxError {
    #[error("invalid host alias: {0}")]
    InvalidHostAlias(String),

    /// Carries the raw input string so diagnostics can show what the user typed.
    #[error("invalid port: {0}")]
    InvalidPort(String),

    #[error("invalid session status: {0}")]
    InvalidSessionStatus(String),

    #[error("home directory not found")]
    HomeDirNotFound,
}
