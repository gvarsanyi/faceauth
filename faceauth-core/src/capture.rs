//! Live camera capture feeding the matcher.

use nokhwa::pixel_format::LumaFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;

use crate::encoding::{Encoding, FaceEncoder};
use crate::error::{FaceAuthError, Result};
use crate::matcher::FaceSource;

/// A streaming camera paired with an encoder.
pub struct LiveSource<'a> {
    camera: Camera,
    encoder: &'a FaceEncoder,
}

impl<'a> LiveSource<'a> {
    /// Open `/dev/videoN` and start streaming. Virtual (frame-injectable)
    /// devices are refused.
    pub fn open(index: u32, encoder: &'a FaceEncoder) -> Result<Self> {
        if faceauth_common::v4l::is_virtual(index) {
            return Err(FaceAuthError::Camera(format!("/dev/video{index} is a virtual camera")));
        }
        // LumaFormat accepts both greyscale (IR) and colour sources.
        let requested = RequestedFormat::new::<LumaFormat>(RequestedFormatType::None);
        let mut camera = Camera::new(CameraIndex::Index(index), requested)
            .map_err(|e| FaceAuthError::Camera(e.to_string()))?;
        camera.open_stream().map_err(|e| FaceAuthError::Camera(e.to_string()))?;
        Ok(Self { camera, encoder })
    }
}

impl Drop for LiveSource<'_> {
    fn drop(&mut self) {
        let _ = self.camera.stop_stream();
    }
}

/// Mean brightness (0-255) below which a frame is treated as dark. IR
/// cameras with a strobing emitter deliver fully black (mean 0) frames
/// between lit ones.
const DARK_MEAN: u64 = 8;

fn is_dark(luma: &[u8]) -> bool {
    !luma.is_empty() && luma.iter().map(|&v| u64::from(v)).sum::<u64>() < DARK_MEAN * luma.len() as u64
}

impl FaceSource for LiveSource<'_> {
    fn next_faces(&mut self) -> Result<Option<Vec<Encoding>>> {
        let buf = self.camera.frame().map_err(|e| FaceAuthError::Camera(e.to_string()))?;
        let luma = buf.decode_image::<LumaFormat>().map_err(|e| FaceAuthError::Camera(e.to_string()))?;
        if is_dark(luma.as_raw()) {
            return Ok(None);
        }
        let (width, height) = (luma.width(), luma.height());
        // dlib expects packed RGB888; replicate the luma channel.
        let rgb: Vec<u8> = luma.into_raw().iter().flat_map(|&g| [g, g, g]).collect();
        self.encoder.encode_faces(&rgb, width, height).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_detection() {
        assert!(is_dark(&[0; 64]));
        assert!(is_dark(&[7; 64]));
        assert!(!is_dark(&[8; 64]));
        assert!(!is_dark(&[47; 64]));
        assert!(!is_dark(&[]));
    }
}
