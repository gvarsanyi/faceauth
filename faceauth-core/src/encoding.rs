use std::path::PathBuf;

use dlib_face_recognition::{
    FaceDetector, FaceDetectorTrait, FaceEncoderNetwork, FaceEncoderTrait, FaceEncoding,
    ImageMatrix, LandmarkPredictor, LandmarkPredictorTrait,
};

use crate::error::{FaceAuthError, Result};

/// Euclidean distance below which two encodings are considered the same person.
/// 0.6 matches the threshold used in dlib's own documentation.
pub const DISTANCE_THRESHOLD: f64 = 0.6;

/// The dlib model files are embedded in the binary at compile time by build.rs,
/// which downloads them from dlib.net if not present under `faceauth-core/models/`.
static SHAPE_PREDICTOR_BYTES: &[u8] =
    include_bytes!("../models/shape_predictor_5_face_landmarks.dat");
static RECOGNITION_MODEL_BYTES: &[u8] =
    include_bytes!("../models/dlib_face_recognition_resnet_model_v1.dat");

/// Directory where embedded model bytes are extracted so dlib can open them.
/// `/tmp` is world-writable, cleaned on reboot, and appropriate for non-secret
/// model weights.  A versioned subdirectory prevents stale-file collisions
/// across binary updates.
const EXTRACT_DIR: &str = concat!("/tmp/faceauth-models-", env!("CARGO_PKG_VERSION"));

/// Extract an embedded model file to `EXTRACT_DIR/<filename>` if it is not
/// already there, and return the path.
fn extract_model(filename: &str, data: &[u8]) -> Result<PathBuf> {
    let dir = std::path::Path::new(EXTRACT_DIR);
    std::fs::create_dir_all(dir).map_err(|e| {
        FaceAuthError::Dlib(format!("failed to create model extraction dir: {e}"))
    })?;

    let path = dir.join(filename);
    if !path.exists() {
        std::fs::write(&path, data).map_err(|e| {
            FaceAuthError::Dlib(format!("failed to extract {filename}: {e}"))
        })?;
    }
    Ok(path)
}

/// Loaded dlib model handles.
///
/// Construction is expensive (reads ~30 MB of model data from disk and
/// initialises the network). Create once per process and reuse.
pub struct FaceEncoder {
    detector: FaceDetector,
    predictor: LandmarkPredictor,
    encoder: FaceEncoderNetwork,
}

impl FaceEncoder {
    /// Initialise the face encoder using the models embedded in the binary.
    ///
    /// On first call the model bytes are extracted to `EXTRACT_DIR`; subsequent
    /// calls (same binary version) reuse the already-extracted files.
    ///
    /// # Errors
    /// Returns [`FaceAuthError::Dlib`] if the model files cannot be extracted
    /// or loaded by dlib.
    pub fn new() -> Result<Self> {
        let detector = FaceDetector::new();

        let predictor_path = extract_model(
            "shape_predictor_5_face_landmarks.dat",
            SHAPE_PREDICTOR_BYTES,
        )?;
        let predictor = LandmarkPredictor::open(&predictor_path)
            .map_err(FaceAuthError::Dlib)?;

        let encoder_path = extract_model(
            "dlib_face_recognition_resnet_model_v1.dat",
            RECOGNITION_MODEL_BYTES,
        )?;
        let encoder = FaceEncoderNetwork::open(&encoder_path)
            .map_err(FaceAuthError::Dlib)?;

        Ok(Self {
            detector,
            predictor,
            encoder,
        })
    }

    /// Detect all faces in an RGB frame and return their 128-D encodings.
    ///
    /// `data` must be packed RGB888 (3 bytes per pixel, row-major).
    /// Returns an empty `Vec` if no faces are detected.
    ///
    /// # Errors
    /// Returns [`FaceAuthError::Dlib`] if an encoding has an unexpected
    /// dimensionality (should never happen with a valid dlib build).
    pub fn encode_faces(&self, data: &[u8], width: u32, height: u32) -> Result<Vec<[f32; 128]>> {
        let matrix = unsafe { ImageMatrix::new(width as usize, height as usize, data.as_ptr()) };

        let locations = self.detector.face_locations(&matrix);

        if locations.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(locations.len());

        for location in locations.iter() {
            let landmarks = self
                .predictor
                .face_landmarks(&matrix, location);

            let encodings = self
                .encoder
                .get_face_encodings(&matrix, &[landmarks], 0);

            for encoding in encodings.iter() {
                results.push(Self::encoding_to_array(encoding)?);
            }
        }

        Ok(results)
    }

    /// Convert a dlib `FaceEncoding` (128 × f64) to a `[f32; 128]`.
    fn encoding_to_array(enc: &FaceEncoding) -> Result<[f32; 128]> {
        let slice: &[f64] = enc.as_ref();
        if slice.len() != 128 {
            return Err(FaceAuthError::Dlib(format!(
                "expected 128-dimensional encoding, got {}",
                slice.len()
            )));
        }
        let mut arr = [0f32; 128];
        for (dst, &src) in arr.iter_mut().zip(slice.iter()) {
            *dst = src as f32;
        }
        Ok(arr)
    }

    /// Euclidean (L2) distance between two 128-D face encodings.
    pub fn distance(a: &[f32; 128], b: &[f32; 128]) -> f64 {
        a.iter()
            .zip(b.iter())
            .map(|(&x, &y)| {
                let d = (x as f64) - (y as f64);
                d * d
            })
            .sum::<f64>()
            .sqrt()
    }

    /// Returns true if `live` is within `DISTANCE_THRESHOLD` of any
    /// encoding stored in `model_encodings`.
    pub fn matches_model(live: &[f32; 128], model_encodings: &[[f32; 128]]) -> bool {
        model_encodings
            .iter()
            .any(|stored| Self::distance(live, stored) < DISTANCE_THRESHOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_identical() {
        let a = [0.5f32; 128];
        assert!(FaceEncoder::distance(&a, &a) < 1e-9);
    }

    #[test]
    fn distance_above_threshold() {
        let a = [0.0f32; 128];
        let b = [1.0f32; 128];
        // distance = sqrt(128 * 1^2) = sqrt(128) ≈ 11.3, well above 0.6
        assert!(FaceEncoder::distance(&a, &b) > DISTANCE_THRESHOLD);
    }

    #[test]
    fn matches_model_hit() {
        let enc = [0.1f32; 128];
        let stored = vec![enc];
        assert!(FaceEncoder::matches_model(&enc, &stored));
    }

    #[test]
    fn matches_model_miss() {
        let a = [0.0f32; 128];
        let b = [1.0f32; 128];
        assert!(!FaceEncoder::matches_model(&a, &[b]));
    }

    #[test]
    fn distance_symmetric() {
        let a = [0.1f32; 128];
        let mut b = [0.5f32; 128];
        b[0] = 0.0;
        assert!((FaceEncoder::distance(&a, &b) - FaceEncoder::distance(&b, &a)).abs() < 1e-9);
    }

    #[test]
    fn distance_known_value() {
        // Only one component differs by 3; distance = sqrt(3²) = 3.0 exactly.
        let mut a = [0.0f32; 128];
        let b = [0.0f32; 128];
        a[0] = 3.0;
        assert!((FaceEncoder::distance(&a, &b) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn matches_model_empty_slice() {
        let live = [0.1f32; 128];
        assert!(!FaceEncoder::matches_model(&live, &[]));
    }

    #[test]
    fn matches_model_multiple_one_hit() {
        let live = [0.1f32; 128];
        let miss = [1.0f32; 128]; // distance ≈ 11.3, well above threshold
        let hit = live;           // distance = 0.0
        // one miss followed by a hit — any() should still return true
        assert!(FaceEncoder::matches_model(&live, &[miss, hit]));
    }
}
