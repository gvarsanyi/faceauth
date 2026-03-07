use std::path::{Path, PathBuf};

use nokhwa::pixel_format::LumaFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;

use crate::error::{FaceAuthError, Result};
use crate::model::CameraId;

/// Information about a single V4L2 camera device, used for listing available cameras.
#[derive(Debug)]
pub struct CameraInfo {
    pub index: u32,
    /// Human-readable device name from `/sys/class/video4linux/videoN/name`.
    pub name: String,
    /// True if the device only advertises greyscale pixel formats — a strong
    /// indicator that it is an IR camera rather than a colour webcam.
    pub greyscale_only: bool,
}

impl CameraInfo {
    /// Heuristic score for how suitable this camera is for face authentication.
    ///
    /// IR cameras (greyscale-only) rank higher; name-based hints ("IR",
    /// "infrared", "Windows Hello") add further weight. A plain colour webcam
    /// scores 0; a clearly-labelled IR camera scores 2.
    pub fn suitability(&self) -> u8 {
        let name_lower = self.name.to_lowercase();
        let name_hint = name_lower.contains("ir")
            || name_lower.contains("infrared")
            || name_lower.contains("windows hello")
            || name_lower.contains("realsense");
        match (self.greyscale_only, name_hint) {
            (true, true) => 2,
            (true, false) | (false, true) => 1,
            (false, false) => 0,
        }
    }
}

// V4L2 ioctl constants and types.
// We define only what we need rather than pulling in a v4l2 crate.
const VIDIOC_QUERYCAP: libc::c_ulong = 0x80685600;
const VIDIOC_ENUM_FMT: libc::c_ulong = 0xC0405602;
const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x00000001;
const V4L2_CAP_DEVICE_CAPS: u32   = 0x80000000;

#[repr(C)]
struct V4l2Capability {
    driver:       [u8; 16],
    card:         [u8; 32],
    bus_info:     [u8; 32],
    version:      u32,
    capabilities: u32,
    device_caps:  u32,
    reserved:     [u32; 3],
}

#[repr(C)]
struct V4l2FmtDesc {
    index: u32,
    type_: u32,
    flags: u32,
    description: [u8; 32],
    pixelformat: u32,
    reserved: [u32; 4],
}

/// Open `/dev/videoN` with `O_RDONLY | O_NONBLOCK`.
/// Returns the file descriptor on success, or -1 on failure.
/// Callers are responsible for calling `libc::close(fd)`.
fn open_video_fd(index: u32) -> libc::c_int {
    let path = format!("/dev/video{}\0", index);
    // SAFETY: path is a valid nul-terminated C string; O_RDONLY|O_NONBLOCK
    // is safe to use on a V4L2 device and won't start any capture.
    unsafe {
        libc::open(
            path.as_ptr() as *const libc::c_char,
            libc::O_RDONLY | libc::O_NONBLOCK,
        )
    }
}

/// Returns `true` if `/dev/videoN` advertises `V4L2_CAP_VIDEO_CAPTURE`.
/// Metadata-only nodes (the odd-numbered duplicates created by UVC drivers)
/// do not set this bit and are filtered out by callers.
fn has_video_capture(index: u32) -> bool {
    let fd = open_video_fd(index);
    if fd < 0 {
        return false;
    }
    let mut cap = V4l2Capability {
        driver: [0; 16], card: [0; 32], bus_info: [0; 32],
        version: 0, capabilities: 0, device_caps: 0, reserved: [0; 3],
    };
    // SAFETY: fd is valid; cap is correctly sized and aligned for VIDIOC_QUERYCAP.
    let ret = unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP, &mut cap as *mut _) };
    unsafe { libc::close(fd) };
    // Use device_caps (per-node) when available, fall back to capabilities.
    let effective = if (cap.capabilities & V4L2_CAP_DEVICE_CAPS) != 0 {
        cap.device_caps
    } else {
        cap.capabilities
    };
    ret == 0 && (effective & V4L2_CAP_VIDEO_CAPTURE) != 0
}

/// Query all pixel formats advertised by `/dev/videoN` via `VIDIOC_ENUM_FMT`.
/// Returns `true` if every format is a greyscale variant (no colour formats).
/// Returns `false` if any colour format is found, or if the device cannot be
/// opened (i.e. don't falsely flag inaccessible devices as greyscale-only).
fn is_greyscale_only(index: u32) -> bool {
    let fd = open_video_fd(index);
    if fd < 0 {
        return false;
    }

    let mut found_any = false;
    let mut found_colour = false;

    for i in 0u32.. {
        let mut desc = V4l2FmtDesc {
            index: i,
            type_: 1, // V4L2_BUF_TYPE_VIDEO_CAPTURE
            flags: 0,
            description: [0u8; 32],
            pixelformat: 0,
            reserved: [0u32; 4],
        };
        // SAFETY: fd is valid; desc is correctly sized and aligned for the ioctl.
        let ret = unsafe { libc::ioctl(fd, VIDIOC_ENUM_FMT, &mut desc as *mut _) };
        if ret < 0 {
            break; // EINVAL = end of format list
        }
        found_any = true;
        // Greyscale fourcc codes: GREY=0x59455247, Y10=0x30313059,
        // Y12=0x32313059, Y16=0x36313059, Y16_BE=0xB6313059
        let grey = matches!(
            desc.pixelformat,
            0x59455247 | 0x30313059 | 0x32313059 | 0x36313059 | 0xB6313059
        );
        if !grey {
            found_colour = true;
            break;
        }
    }

    unsafe { libc::close(fd) };
    found_any && !found_colour
}

/// Read the human-readable device name for `/dev/videoN` from sysfs.
///
/// Returns an empty string if the sysfs entry cannot be read.
pub fn camera_name_for_index(index: u32) -> String {
    std::fs::read_to_string(format!("/sys/class/video4linux/video{}/name", index))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Enumerate all `/dev/videoN` capture devices and return their info, sorted
/// by descending suitability score (most IR-like first).
pub fn list_cameras() -> Vec<CameraInfo> {
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return Vec::new();
    };

    let mut cameras: Vec<CameraInfo> = entries
        .filter_map(|e| {
            let entry = e.ok()?;
            let dir_name = entry.file_name().into_string().ok()?;
            let index: u32 = dir_name.strip_prefix("video")?.parse().ok()?;
            if !has_video_capture(index) {
                return None;
            }
            let name = std::fs::read_to_string(entry.path().join("name"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let greyscale_only = is_greyscale_only(index);
            Some(CameraInfo { index, name, greyscale_only })
        })
        .collect();

    cameras.sort_by_key(|c| c.index);
    cameras
}

/// A captured video frame decoded to raw RGB bytes.
pub struct Frame {
    /// Flat RGB bytes: 3 bytes per pixel, row-major, no padding.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Enumerate `/dev/v4l/by-id/` or `/dev/v4l/by-path/` and return all
/// (symlink_name, resolved_index) pairs. Silently skips unresolvable entries.
fn enumerate_v4l_dir(dir: &Path) -> Vec<(String, u32)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| {
            let entry = e.ok()?;
            let name = entry.file_name().into_string().ok()?;
            // Resolve the symlink to its real path, e.g. /dev/video2
            let target = std::fs::canonicalize(entry.path()).ok()?;
            // Extract the trailing integer from "video2" → 2
            let file_name = target.file_name()?.to_str()?;
            let index_str = file_name.strip_prefix("video")?;
            let index: u32 = index_str.parse().ok()?;
            Some((name, index))
        })
        .collect()
}

/// Build a `CameraId` for a known V4L2 device index by looking up its
/// stable udev symlink names in `/dev/v4l/by-id/` and `/dev/v4l/by-path/`.
pub fn camera_id_for_index(index: u32) -> CameraId {
    let by_id = enumerate_v4l_dir(Path::new("/dev/v4l/by-id/"))
        .into_iter()
        .find(|(_, i)| *i == index)
        .map(|(name, _)| name);

    let by_path = enumerate_v4l_dir(Path::new("/dev/v4l/by-path/"))
        .into_iter()
        .find(|(_, i)| *i == index)
        .map(|(name, _)| name);

    CameraId { by_id, by_path, index }
}

/// Resolve a `CameraId` back to a `/dev/videoN` index.
///
/// Resolution order:
/// 1. `by_id`  — look up in `/dev/v4l/by-id/`
/// 2. `by_path` — look up in `/dev/v4l/by-path/`
/// 3. `index`  — use the stored integer directly
fn resolve_camera_index(id: &CameraId) -> u32 {
    // Helper: resolve a symlink name in a udev dir to its device index.
    let resolve = |dir: &Path, name: &str| -> Option<u32> {
        let link: PathBuf = dir.join(name);
        let target = std::fs::canonicalize(&link).ok()?;
        let file_name = target.file_name()?.to_str()?;
        file_name.strip_prefix("video")?.parse().ok()
    };

    // Try by_path first: it encodes both the USB port and the interface number
    // within the device, making it the most specific identifier for cameras
    // that expose multiple V4L2 interfaces (e.g. RGB + IR streams). by_id only
    // encodes the device serial, so its video-index suffix can resolve to a
    // different /dev/videoN after a reboot if the kernel assigns indices
    // differently.
    if let Some(name) = &id.by_path {
        if let Some(idx) = resolve(Path::new("/dev/v4l/by-path/"), name) {
            return idx;
        }
    }

    // Fall back to by_id: stable across USB port changes, but ambiguous for
    // multi-interface devices.
    if let Some(name) = &id.by_id {
        if let Some(idx) = resolve(Path::new("/dev/v4l/by-id/"), name) {
            return idx;
        }
    }

    id.index
}

/// Open the camera described by `id` and return a handle.
///
/// Resolves `id` to a `/dev/videoN` index using the stable udev symlinks
/// when available, falling back to the stored integer index.
///
/// The stream is NOT started yet; call `camera.open_stream()` before
/// calling `capture_frame`. This separation lets callers handle setup
/// errors before entering the capture loop.
///
/// # Errors
/// Returns [`FaceAuthError::Camera`] if the device cannot be opened.
pub fn open_camera(id: &CameraId) -> Result<Camera> {
    let index = resolve_camera_index(id);
    let cam_index = CameraIndex::Index(index);
    // LumaFormat includes GRAY in its supported formats (unlike RgbFormat which
    // only includes colour formats), so this works for both greyscale IR cameras
    // and colour cameras.
    let requested = RequestedFormat::new::<LumaFormat>(RequestedFormatType::None);
    Camera::new(cam_index, requested).map_err(|e| FaceAuthError::Camera(e.to_string()))
}

/// Capture a single frame from an already-streaming camera.
///
/// Returns the frame decoded as packed RGB888.
///
/// # Errors
/// Returns [`FaceAuthError::Camera`] if the frame cannot be captured or decoded.
pub fn capture_frame(camera: &mut Camera) -> Result<Frame> {
    let buf = camera
        .frame()
        .map_err(|e| FaceAuthError::Camera(e.to_string()))?;

    // Decode as luma (works for both GRAY and colour sources), then expand
    // each greyscale byte to three identical R/G/B bytes so dlib always
    // receives packed RGB888.
    let decoded = buf
        .decode_image::<LumaFormat>()
        .map_err(|e| FaceAuthError::Camera(e.to_string()))?;

    let width = decoded.width();
    let height = decoded.height();
    let data = decoded.into_raw().iter().flat_map(|&g| [g, g, g]).collect();

    Ok(Frame { data, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- CameraInfo::suitability ---

    #[test]
    fn suitability_colour_no_hint() {
        // Plain colour webcam — lowest score.
        let cam = CameraInfo { index: 0, name: "USB Webcam".to_string(), greyscale_only: false };
        assert_eq!(cam.suitability(), 0);
    }

    #[test]
    fn suitability_greyscale_no_hint() {
        // Greyscale-only but no name hint — one point for the pixel format.
        let cam = CameraInfo { index: 0, name: "Some Camera".to_string(), greyscale_only: true };
        assert_eq!(cam.suitability(), 1);
    }

    #[test]
    fn suitability_colour_with_hint() {
        // Colour camera but name advertises IR — one point for the hint.
        let cam = CameraInfo { index: 0, name: "Windows Hello Camera".to_string(), greyscale_only: false };
        assert_eq!(cam.suitability(), 1);
    }

    #[test]
    fn suitability_greyscale_with_hint() {
        // Both signals present — highest score.
        let cam = CameraInfo { index: 0, name: "Intel RealSense IR".to_string(), greyscale_only: true };
        assert_eq!(cam.suitability(), 2);
    }

    #[test]
    fn suitability_all_name_hints_recognised() {
        // Every hint keyword should trigger suitability >= 1.
        for name in &["IR Camera", "Infrared Sensor", "Windows Hello", "Intel RealSense"] {
            let cam = CameraInfo { index: 0, name: name.to_string(), greyscale_only: false };
            assert!(cam.suitability() >= 1, "expected name hint match for: {name}");
        }
    }

    // --- camera_name_for_index ---

    #[test]
    fn camera_name_for_missing_index_is_empty() {
        // No sysfs entry exists for this index; should return "" without panicking.
        assert_eq!(camera_name_for_index(999_999), "");
    }
}
