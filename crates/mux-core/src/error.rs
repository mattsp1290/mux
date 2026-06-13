use thiserror::Error;

#[derive(Debug, Error)]
pub enum MuxError {
    #[error("invalid host alias: {0}")]
    InvalidHostAlias(String),

    #[error("invalid port: {0}")]
    InvalidPort(u16),

    #[error("invalid session status: {0}")]
    InvalidSessionStatus(String),

    #[error("home directory not found")]
    HomeDirNotFound,

    #[error("command context: {context}: {source}")]
    CommandContext {
        context: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
