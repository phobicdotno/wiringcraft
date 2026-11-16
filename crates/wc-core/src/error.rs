use std::path::PathBuf;

/// Errors from reading, writing or validating project data.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid TOML in {path}: {message}")]
    Toml { path: PathBuf, message: String },
    #[error("invalid device tag {0:?}: {1}")]
    Tag(String, &'static str),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}
