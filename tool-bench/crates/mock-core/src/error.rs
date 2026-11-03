use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid RFC 3339 timestamp {value:?}: {source}")]
    Timestamp {
        value: String,
        #[source]
        source: time::error::Parse,
    },
    #[error("reading {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    #[error("dataset {}: {message}", path.display())]
    Dataset { path: PathBuf, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;
