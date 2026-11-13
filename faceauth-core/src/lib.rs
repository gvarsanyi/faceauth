//! Face detection, encoding, matching, and model storage for the faceauth
//! daemon. Only the daemon links this crate.

pub mod capture;
pub mod encoding;
pub mod error;
pub mod matcher;
pub mod store;

pub use encoding::{Encoding, FaceEncoder};
pub use error::{FaceAuthError, Result};
