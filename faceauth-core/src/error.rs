use thiserror::Error;

/// All errors that can be returned by faceauth-core operations.
#[derive(Debug, Error)]
pub enum FaceAuthError {
    #[error("Camera error: {0}")]
    Camera(String),

    #[error("Model storage error: {0}")]
    Storage(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("No face model found for user '{0}'")]
    ModelNotFound(String),

    #[error("Authentication timed out after {0} seconds")]
    Timeout(u64),

    #[error("dlib error: {0}")]
    Dlib(String),
}

pub type Result<T> = std::result::Result<T, FaceAuthError>;
