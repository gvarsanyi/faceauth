use thiserror::Error;

/// Errors returned by faceauth-core operations.
#[derive(Debug, Error)]
pub enum FaceAuthError {
    #[error("camera error: {0}")]
    Camera(String),

    #[error("storage error: {0}")]
    Storage(#[from] std::io::Error),

    #[error("invalid model file: {0}")]
    InvalidModel(String),

    #[error("no face model enrolled")]
    NoModel,

    /// `closest` is the smallest distance seen for a single-face frame; it is
    /// for the daemon log only (a similarity score helps an attacker tune a
    /// spoof) and is not part of the message.
    #[error("no matching face within {secs} seconds")]
    NoMatch { secs: u64, closest: Option<f64> },

    #[error("no single face detected within {0} seconds")]
    NoFace(u64),

    #[error("face recognition error: {0}")]
    Dlib(String),

    #[error("cancelled")]
    Cancelled,
}

impl From<serde_json::Error> for FaceAuthError {
    fn from(e: serde_json::Error) -> Self {
        FaceAuthError::InvalidModel(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, FaceAuthError>;
