//! The faceauth D-Bus contract: names, paths, errors, and polkit action IDs.
//!
//! The daemon owns `BUS_NAME` on the system bus and exports a single object
//! at `OBJECT_PATH` implementing `INTERFACE`. See `data/dbus/` in the repository
//! for the introspection XML and bus policy.

/// Well-known bus name owned by faceauth-daemon.
pub const BUS_NAME: &str = "io.github.gvarsanyi.FaceAuth1";
/// Path of the manager object.
pub const OBJECT_PATH: &str = "/io/github/gvarsanyi/FaceAuth1";
/// Manager interface name.
pub const INTERFACE: &str = "io.github.gvarsanyi.FaceAuth1.Manager";
/// Session-bus name owned by the running faceauth-agent (single instance).
pub const AGENT_BUS_NAME: &str = "io.github.gvarsanyi.FaceAuth1.Agent";

/// Fixed system bus socket. Clients that may run inside setuid processes
/// (the PAM module) must use this path and never `DBUS_SYSTEM_BUS_ADDRESS`,
/// which the invoking user controls.
pub const SYSTEM_BUS_ADDRESS: &str = "unix:path=/run/dbus/system_bus_socket";

/// polkit action: manage your own face model and service settings.
pub const ACTION_MANAGE_OWN: &str = "io.github.gvarsanyi.faceauth.manage-own";
/// polkit action: manage another user's face model and service settings.
pub const ACTION_MANAGE_OTHER: &str = "io.github.gvarsanyi.faceauth.manage-other";

/// Sentinel camera index for `Enroll`: reuse the camera of the existing model.
pub const ENROLLED_CAMERA: i32 = -1;

/// Camera descriptor returned by `ListCameras`: (index, name, suitability).
pub type CameraTuple = (u32, String, u8);

/// `ListCameras` suitability levels.
pub const SUITABILITY_COLOUR: u8 = 0;
pub const SUITABILITY_POSSIBLE_IR: u8 = 1;
/// Greyscale-only and named like an IR camera: recommended.
pub const SUITABILITY_IR: u8 = 2;

/// `EnrollProgress` status values.
pub const STATUS_CAPTURED: &str = "captured";
pub const STATUS_TOO_SIMILAR: &str = "too-similar";

/// Position in `cameras` of the default camera: the most suitable one,
/// the lowest `/dev/videoN` among equals. `None` if the list is empty.
pub fn preferred_camera(cameras: &[CameraTuple]) -> Option<usize> {
    cameras
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.2.cmp(&b.2).then(b.0.cmp(&a.0)))
        .map(|(i, _)| i)
}

/// Errors returned by the daemon. Each variant maps to the D-Bus error name
/// `io.github.gvarsanyi.FaceAuth1.Error.<Variant>`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "io.github.gvarsanyi.FaceAuth1.Error", impl_display = false)]
pub enum Error {
    /// Transport-level failure (daemon unreachable, bus error, timeout).
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The caller may not perform this operation on this user.
    PermissionDenied(String),
    /// polkit authorization was refused or dismissed.
    NotAuthorized(String),
    /// The user has no enrolled face model.
    NoModel(String),
    /// The PAM service is not allowed for face authentication.
    ServiceNotAllowed(String),
    /// The caller is not in a local, active seat session.
    NotLocalSession(String),
    /// No matching face was seen before the timeout.
    NoMatch(String),
    /// The camera could not be opened or read.
    Camera(String),
    /// A request argument was invalid.
    InvalidArgs(String),
    /// Too many concurrent requests: an enrollment is already running for
    /// this caller, or the user has too many notification agents.
    Busy(String),
    /// The operation was cancelled.
    Cancelled(String),
    /// Any other daemon-side failure.
    Failed(String),
}

impl Error {
    /// True if the error means the daemon could not be reached at all.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Error::ZBus(_))
    }

    /// Human-readable message without the D-Bus error name.
    pub fn message(&self) -> String {
        match self {
            Error::ZBus(e) => format!("faceauth daemon not available: {e}"),
            Error::PermissionDenied(m)
            | Error::NotAuthorized(m)
            | Error::NoModel(m)
            | Error::ServiceNotAllowed(m)
            | Error::NotLocalSession(m)
            | Error::NoMatch(m)
            | Error::Camera(m)
            | Error::InvalidArgs(m)
            | Error::Busy(m)
            | Error::Cancelled(m)
            | Error::Failed(m) => m.clone(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_camera_is_most_suitable_then_lowest_index() {
        let cam = |i: u32, s: u8| (i, String::new(), s);
        assert_eq!(preferred_camera(&[]), None);
        assert_eq!(preferred_camera(&[cam(0, 0), cam(2, 2), cam(4, 0)]), Some(1));
        assert_eq!(preferred_camera(&[cam(3, 1), cam(1, 1)]), Some(1));
        assert_eq!(preferred_camera(&[cam(0, 0), cam(2, 0)]), Some(0));
    }
}
