//! V4L2 camera enumeration and stable-identifier resolution (no capture).
//!
//! Only the ioctls needed for discovery are defined here; frame capture lives
//! in faceauth-core so that light-weight clients do not link camera libraries.

use crate::dbus::{SUITABILITY_COLOUR, SUITABILITY_IR, SUITABILITY_POSSIBLE_IR};
use std::path::Path;

/// Information about a V4L2 capture device.
#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub index: u32,
    /// Human-readable device name from sysfs.
    pub name: String,
    /// True if the device only advertises greyscale pixel formats, a strong
    /// indicator of an IR camera.
    pub greyscale_only: bool,
}

impl CameraInfo {
    /// Heuristic suitability for face authentication (`SUITABILITY_*`).
    pub fn suitability(&self) -> u8 {
        match (self.greyscale_only, name_suggests_ir(&self.name)) {
            (true, true) => SUITABILITY_IR,
            (true, false) | (false, true) => SUITABILITY_POSSIBLE_IR,
            (false, false) => SUITABILITY_COLOUR,
        }
    }
}

/// True if the device name contains an IR hint as a whole word ("IR",
/// "Infrared") or a known IR product name. Whole-word matching avoids false
/// positives such as "Virtual".
fn name_suggests_ir(name: &str) -> bool {
    let lower = name.to_lowercase();
    let word_hit = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == "ir" || w == "infrared");
    word_hit || lower.contains("windows hello") || lower.contains("realsense")
}

// V4L2 ioctl numbers and structures (linux/videodev2.h).
const VIDIOC_QUERYCAP: libc::c_ulong = 0x80685600;
const VIDIOC_ENUM_FMT: libc::c_ulong = 0xC0405602;
const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x00000001;
const V4L2_CAP_DEVICE_CAPS: u32 = 0x80000000;
const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
/// Greyscale fourcc codes: GREY, Y10, Y12, Y16, Y16_BE.
const GREY_FORMATS: [u32; 5] = [0x59455247, 0x30313059, 0x32313059, 0x36313059, 0xB6313059];
/// Drivers that expose synthetic or user-writable video devices. Frames from
/// these can be injected by unprivileged processes, so they are never used.
const VIRTUAL_DRIVERS: [&str; 3] = ["v4l2 loopback", "akvcam", "vivid"];

#[repr(C)]
struct V4l2Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
struct V4l2FmtDesc {
    index: u32,
    type_: u32,
    flags: u32,
    description: [u8; 32],
    pixelformat: u32,
    mbus_code: u32,
    reserved: [u32; 3],
}

/// An open, non-blocking, read-only `/dev/videoN` descriptor.
struct VideoFd(libc::c_int);

impl VideoFd {
    fn open(index: u32) -> Option<VideoFd> {
        let path = std::ffi::CString::new(format!("/dev/video{index}")).ok()?;
        // SAFETY: valid C string; O_NONBLOCK|O_RDONLY does not start capture.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        (fd >= 0).then_some(VideoFd(fd))
    }

    fn query_cap(&self) -> Option<V4l2Capability> {
        // SAFETY: plain-old-data struct; zeroed is a valid bit pattern.
        let mut cap: V4l2Capability = unsafe { std::mem::zeroed() };
        // SAFETY: fd is valid; cap matches the kernel layout for QUERYCAP.
        let ret = unsafe { libc::ioctl(self.0, VIDIOC_QUERYCAP, &mut cap as *mut V4l2Capability) };
        (ret == 0).then_some(cap)
    }
}

impl Drop for VideoFd {
    fn drop(&mut self) {
        // SAFETY: we own the descriptor.
        unsafe { libc::close(self.0) };
    }
}

fn c_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Driver name of `/dev/videoN`, if it can be queried.
fn driver_name(index: u32) -> Option<String> {
    VideoFd::open(index)?.query_cap().map(|c| c_str(&c.driver))
}

/// True if `/dev/videoN` is served by a virtual (frame-injectable) driver.
pub fn is_virtual(index: u32) -> bool {
    driver_name(index).is_some_and(|d| VIRTUAL_DRIVERS.contains(&d.as_str()))
}

/// True if `/dev/videoN` is a real video-capture node (not metadata-only).
fn has_video_capture(fd: &VideoFd) -> bool {
    let Some(cap) = fd.query_cap() else { return false };
    let effective = if cap.capabilities & V4L2_CAP_DEVICE_CAPS != 0 { cap.device_caps } else { cap.capabilities };
    effective & V4L2_CAP_VIDEO_CAPTURE != 0
}

/// True if every pixel format advertised by the device is greyscale.
fn greyscale_only(fd: &VideoFd) -> bool {
    let mut found_any = false;
    for i in 0u32..64 {
        // SAFETY: plain-old-data struct; zeroed is a valid bit pattern.
        let mut desc: V4l2FmtDesc = unsafe { std::mem::zeroed() };
        desc.index = i;
        desc.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        // SAFETY: fd is valid; desc matches the kernel layout for ENUM_FMT.
        if unsafe { libc::ioctl(fd.0, VIDIOC_ENUM_FMT, &mut desc as *mut V4l2FmtDesc) } < 0 {
            break; // EINVAL marks the end of the list
        }
        if !GREY_FORMATS.contains(&desc.pixelformat) {
            return false;
        }
        found_any = true;
    }
    found_any
}

/// True if `/dev/videoN` advertises only greyscale formats.
pub fn is_greyscale_only(index: u32) -> bool {
    VideoFd::open(index).is_some_and(|fd| greyscale_only(&fd))
}

/// Device name of `/dev/videoN` from sysfs, or an empty string.
pub fn camera_name(index: u32) -> String {
    std::fs::read_to_string(format!("/sys/class/video4linux/video{index}/name"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// All usable capture devices, sorted by index. Virtual drivers are excluded.
pub fn list_cameras() -> Vec<CameraInfo> {
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return Vec::new();
    };
    let mut cameras: Vec<CameraInfo> = entries
        .filter_map(|e| {
            let index: u32 = e.ok()?.file_name().to_str()?.strip_prefix("video")?.parse().ok()?;
            let fd = VideoFd::open(index)?;
            let cap = fd.query_cap()?;
            if VIRTUAL_DRIVERS.contains(&c_str(&cap.driver).as_str()) || !has_video_capture(&fd) {
                return None;
            }
            Some(CameraInfo { index, name: camera_name(index), greyscale_only: greyscale_only(&fd) })
        })
        .collect();
    cameras.sort_by_key(|c| c.index);
    cameras
}

/// Parse "videoN" (a device file name) into N.
fn video_index(file_name: &str) -> Option<u32> {
    file_name.strip_prefix("video")?.parse().ok()
}

/// Resolve `dir/name` (a udev symlink) to a `/dev/videoN` index.
fn resolve_link(dir: &Path, name: &str) -> Option<u32> {
    if name.contains('/') {
        return None;
    }
    let target = std::fs::canonicalize(dir.join(name)).ok()?;
    video_index(target.file_name()?.to_str()?)
}

/// Stable identifier for a device index: the `/dev/v4l/by-path/` name, else
/// the `/dev/v4l/by-id/` name, else `"/dev/videoN"`.
pub fn camera_id_for_index(index: u32) -> String {
    for dir in ["/dev/v4l/by-path", "/dev/v4l/by-id"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            if resolve_link(Path::new(dir), &name) == Some(index) {
                return name;
            }
        }
    }
    format!("/dev/video{index}")
}

/// Resolve a stored camera identifier back to the current `/dev/videoN`
/// index. Returns `None` if the device is not present; callers must not
/// fall back to a different camera.
pub fn resolve_camera(id: &str) -> Option<u32> {
    if let Some(i) = id.strip_prefix("/dev/").and_then(video_index) {
        return Path::new(&format!("/dev/video{i}")).exists().then_some(i);
    }
    resolve_link(Path::new("/dev/v4l/by-path"), id).or_else(|| resolve_link(Path::new("/dev/v4l/by-id"), id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(name: &str, grey: bool) -> CameraInfo {
        CameraInfo { index: 0, name: name.to_string(), greyscale_only: grey }
    }

    #[test]
    fn suitability_levels() {
        assert_eq!(cam("USB Webcam", false).suitability(), 0);
        assert_eq!(cam("Some Camera", true).suitability(), 1);
        assert_eq!(cam("Windows Hello Camera", false).suitability(), 1);
        assert_eq!(cam("ASUS FHD webcam: ASUS IR camera", true).suitability(), 2);
    }

    #[test]
    fn ir_hint_is_whole_word() {
        for name in ["IR Camera", "Infrared Sensor", "Intel RealSense", "cam-ir"] {
            assert!(name_suggests_ir(name), "{name}");
        }
        for name in ["OBS Virtual Camera", "Firewire cam", "HD Pro Webcam C920", "Logitech BRIO"] {
            assert!(!name_suggests_ir(name), "{name}");
        }
    }

    #[test]
    fn video_index_parsing() {
        assert_eq!(video_index("video12"), Some(12));
        assert_eq!(video_index("video"), None);
        assert_eq!(video_index("vbi0"), None);
    }

    #[test]
    fn resolve_rejects_missing_and_traversal() {
        assert_eq!(resolve_camera("/dev/video999999"), None);
        assert_eq!(resolve_camera("no-such-camera-id"), None);
        assert_eq!(resolve_camera("../../video0"), None);
    }

    #[test]
    fn missing_camera_name_is_empty() {
        assert_eq!(camera_name(999_999), "");
    }
}
