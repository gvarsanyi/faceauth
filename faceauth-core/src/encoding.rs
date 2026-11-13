//! dlib face detection and 128-D descriptor extraction.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};

use dlib_face_recognition::{
    FaceDetector, FaceDetectorTrait, FaceEncoderNetwork, FaceEncoderTrait, FaceEncoding, ImageMatrix,
    LandmarkPredictor, LandmarkPredictorTrait,
};

use crate::error::{FaceAuthError, Result};

/// A 128-dimensional face descriptor.
pub type Encoding = [f32; 128];

/// Model files verified and decompressed by build.rs (see models/README.md).
static SHAPE_PREDICTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shape_predictor_5_face_landmarks.dat"));
static RECOGNITION_MODEL: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/dlib_face_recognition_resnet_model_v1.dat"));

/// Copy `data` into an anonymous memory file. dlib can only load models from
/// a path, so the caller opens `/proc/self/fd/<fd>` while the file is alive.
/// Nothing is written to the filesystem.
fn memfd_with(name: &str, data: &[u8]) -> Result<File> {
    let cname = std::ffi::CString::new(name).map_err(|e| FaceAuthError::Dlib(e.to_string()))?;
    // SAFETY: valid C string; MFD_CLOEXEC keeps the fd out of child processes.
    let fd = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(FaceAuthError::Storage(std::io::Error::last_os_error()));
    }
    // SAFETY: fd is a fresh descriptor that we own.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(data)?;
    Ok(file)
}

fn fd_path(file: &File) -> String {
    format!("/proc/self/fd/{}", file.as_raw_fd())
}

/// Loaded dlib model handles. Construction parses ~30 MB of model data;
/// create once per process and reuse.
pub struct FaceEncoder {
    detector: FaceDetector,
    predictor: LandmarkPredictor,
    encoder: FaceEncoderNetwork,
}

impl FaceEncoder {
    /// Load the embedded models.
    pub fn new() -> Result<Self> {
        let predictor_file = memfd_with("shape_predictor", SHAPE_PREDICTOR)?;
        let predictor = LandmarkPredictor::open(fd_path(&predictor_file)).map_err(FaceAuthError::Dlib)?;
        let encoder_file = memfd_with("recognition_model", RECOGNITION_MODEL)?;
        let encoder = FaceEncoderNetwork::open(fd_path(&encoder_file)).map_err(FaceAuthError::Dlib)?;
        Ok(Self { detector: FaceDetector::new(), predictor, encoder })
    }

    /// Detect all faces in a packed RGB888 frame and return their encodings.
    pub fn encode_faces(&self, rgb: &[u8], width: u32, height: u32) -> Result<Vec<Encoding>> {
        let expected = width as usize * height as usize * 3;
        if rgb.len() < expected {
            return Err(FaceAuthError::Camera(format!("short frame: {} < {expected} bytes", rgb.len())));
        }
        // SAFETY: `rgb` holds at least width*height*3 bytes (checked above)
        // and outlives `matrix`.
        let matrix = unsafe { ImageMatrix::new(width as usize, height as usize, rgb.as_ptr()) };
        let mut results = Vec::new();
        for location in self.detector.face_locations(&matrix).iter() {
            let landmarks = self.predictor.face_landmarks(&matrix, location);
            for encoding in self.encoder.get_face_encodings(&matrix, &[landmarks], 0).iter() {
                results.push(to_array(encoding)?);
            }
        }
        Ok(results)
    }
}

/// Convert a dlib descriptor (128 x f64) to `[f32; 128]`.
fn to_array(enc: &FaceEncoding) -> Result<Encoding> {
    let slice: &[f64] = enc.as_ref();
    if slice.len() != 128 {
        return Err(FaceAuthError::Dlib(format!("expected 128-dimensional encoding, got {}", slice.len())));
    }
    let mut arr = [0f32; 128];
    for (dst, &src) in arr.iter_mut().zip(slice) {
        *dst = src as f32;
    }
    Ok(arr)
}

/// Euclidean distance between two descriptors.
pub fn distance(a: &Encoding, b: &Encoding) -> f64 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| {
            let d = f64::from(x) - f64::from(y);
            d * d
        })
        .sum::<f64>()
        .sqrt()
}

/// Smallest distance from `live` to any of `stored` (`f64::INFINITY` if empty).
pub fn min_distance(live: &Encoding, stored: &[Encoding]) -> f64 {
    stored.iter().map(|s| distance(live, s)).fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_basics() {
        let a = [0.5f32; 128];
        assert!(distance(&a, &a) < 1e-9);
        let mut b = [0.0f32; 128];
        b[0] = 3.0;
        assert!((distance(&[0.0; 128], &b) - 3.0).abs() < 1e-6);
        assert!((distance(&a, &b) - distance(&b, &a)).abs() < 1e-9);
    }

    #[test]
    fn min_distance_picks_closest() {
        let live = [0.1f32; 128];
        assert_eq!(min_distance(&live, &[]), f64::INFINITY);
        assert!(min_distance(&live, &[[1.0; 128], live]) < 1e-9);
    }

    #[test]
    fn memfd_roundtrip() {
        let file = memfd_with("test", b"hello").unwrap();
        assert_eq!(std::fs::read(fd_path(&file)).unwrap(), b"hello");
    }

    #[test]
    fn embedded_models_are_present() {
        assert!(SHAPE_PREDICTOR.len() > 1_000_000);
        assert!(RECOGNITION_MODEL.len() > 10_000_000);
    }
}
