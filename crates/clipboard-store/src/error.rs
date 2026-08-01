use std::{error::Error, fmt};

#[derive(Debug)]
pub enum StoreError {
    Database(rusqlite::Error),
    Io(std::io::Error),
    ActorStopped,
    InvalidData(&'static str),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "database error: {error}"),
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::ActorStopped => formatter.write_str("store actor stopped"),
            Self::InvalidData(message) => write!(formatter, "invalid stored data: {message}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::ActorStopped | Self::InvalidData(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
