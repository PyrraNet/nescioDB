use std::fmt;

/// nescioDB error type.
///
/// `AxiomConflict` is special: it is not a failure of the database but a
/// property of the data — two axiomatic evidences annihilated a posterior.
/// Real contradiction must surface; it is never renormalized away.
#[derive(Debug)]
pub enum Error {
    AxiomConflict(String),
    Invalid(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::AxiomConflict(m) => write!(f, "axiom conflict: {m}"),
            Error::Invalid(m) => write!(f, "invalid: {m}"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Json(e) => write!(f, "json: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
