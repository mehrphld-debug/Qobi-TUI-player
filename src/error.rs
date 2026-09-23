/// Library errors: `thiserror` for typed failures, converted to `anyhow` at the app edge.
#[derive(thiserror::Error, Debug)]
pub enum QobiError {
    #[error("IO failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("config parse failed: {0}")]
    ConfigParse(String),

    #[error("IPC protocol error: {0}")]
    Ipc(String),

    #[error("Qobi already running but unreachable — remove {0} or kill the stale process")]
    StaleInstance(String),

    #[error("invalid target: {0}")]
    InvalidTarget(String),

    #[error("decode failed: {0}")]
    Decode(String),

    #[error("audio output failed: {0}")]
    Output(String),
}
