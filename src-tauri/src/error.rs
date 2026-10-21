use serde::{Serialize, Serializer};

/// Error type returned by every command. Serialized as `{ kind, message }` so the
/// frontend can branch on `kind` (e.g. show a re-auth prompt on `auth`).
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::NotFound(_) => "notFound",
            AppError::AlreadyExists(_) => "alreadyExists",
            AppError::PermissionDenied(_) => "permissionDenied",
            AppError::Auth(_) => "auth",
            AppError::Connection(_) => "connection",
            AppError::Unsupported(_) => "unsupported",
            AppError::Invalid(_) => "invalid",
            AppError::Cancelled => "cancelled",
            AppError::Other(_) => "other",
        }
    }

    pub fn other(e: impl std::fmt::Display) -> Self {
        AppError::Other(e.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("AppError", 2)?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match e.kind() {
            NotFound => AppError::NotFound(e.to_string()),
            AlreadyExists => AppError::AlreadyExists(e.to_string()),
            PermissionDenied => AppError::PermissionDenied(e.to_string()),
            ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected | TimedOut | BrokenPipe => {
                AppError::Connection(e.to_string())
            }
            _ => AppError::Other(e.to_string()),
        }
    }
}

impl From<opendal::Error> for AppError {
    fn from(e: opendal::Error) -> Self {
        use opendal::ErrorKind as K;
        match e.kind() {
            K::NotFound => AppError::NotFound(e.to_string()),
            K::AlreadyExists => AppError::AlreadyExists(e.to_string()),
            K::PermissionDenied => AppError::PermissionDenied(e.to_string()),
            K::ConfigInvalid => AppError::Invalid(e.to_string()),
            K::Unsupported => AppError::Unsupported(e.to_string()),
            _ => AppError::Other(e.to_string()),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(format!("{e:#}"))
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        AppError::Other(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Other(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
