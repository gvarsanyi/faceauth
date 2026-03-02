use std::process;

use faceauth_core::{list_cameras, CameraInfo};
use gettextrs::gettext;

fn camera_hint(cam: &CameraInfo) -> String {
    match cam.suitability() {
        2 => format!(" [{}]", gettext("IR, recommended")),
        1 => format!(" [{}]", gettext("possible IR")),
        _ => String::new(),
    }
}

fn camera_name(cam: &CameraInfo) -> String {
    if cam.name.is_empty() {
        gettext("(unknown)")
    } else {
        cam.name.clone()
    }
}

/// Print a table of available cameras to stderr and return the list.
pub fn print_camera_list() -> Vec<CameraInfo> {
    let cameras = list_cameras();
    let mut sorted: Vec<&CameraInfo> = cameras.iter().collect();
    sorted.sort_by_key(|c| c.index);
    eprintln!("{}", gettext("Available cameras:"));
    for cam in &sorted {
        eprintln!(
            "  /dev/video{}  {}{}",
            cam.index,
            camera_name(cam),
            camera_hint(cam)
        );
    }
    cameras
}

/// Returns true if stdin is connected to a terminal (i.e. a user can type).
fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) != 0 }
}

/// Prompt the user to choose a camera interactively.
///
/// Lists cameras, shows the recommended default, reads a line from stdin.
/// An empty line (just Enter) accepts the default.
fn prompt_camera(cameras: &[CameraInfo]) -> u32 {
    let default = &cameras[0]; // list_cameras() sorts by suitability descending — best choice

    // Display in natural /dev/videoN order for readability.
    let mut sorted: Vec<&CameraInfo> = cameras.iter().collect();
    sorted.sort_by_key(|c| c.index);

    eprintln!("{}", gettext("Available cameras:"));
    for cam in sorted.iter() {
        let marker = if cam.index == default.index { "*" } else { " " };
        eprintln!(
            "  {} {}.  /dev/video{}  {}{}",
            marker,
            cam.index,
            cam.index,
            camera_name(cam),
            camera_hint(cam)
        );
    }

    let valid_indices: Vec<u32> = sorted.iter().map(|c| c.index).collect();
    eprintln!();
    eprint!(
        "{}",
        gettext("Select camera (default {index}): ").replace("{index}", &default.index.to_string())
    );

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_ok() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            if let Ok(n) = trimmed.parse::<u32>() {
                if valid_indices.contains(&n) {
                    return n;
                }
            }
            eprintln!(
                "faceauth: {}",
                gettext("invalid selection '{input}'; expected one of: {options}")
                    .replace("{input}", trimmed)
                    .replace(
                        "{options}",
                        &valid_indices
                            .iter()
                            .map(|n| n.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
            );
            process::exit(1);
        }
    }
    // Empty input or read error → use the default.
    default.index
}

/// Resolve a filesystem path (or symlink) to a `/dev/videoN` index.
/// Returns `None` if the path doesn't resolve to a `videoN` device.
fn resolve_video_symlink(path: &std::path::Path) -> Option<u32> {
    let real = std::fs::canonicalize(path).ok()?;
    let name = real.file_name()?.to_str()?;
    let idx_str = name.strip_prefix("video")?;
    idx_str.parse().ok()
}

/// Resolve a `--camera` argument to a V4L2 device index.
///
/// Accepted forms:
/// - bare integer: `0`, `2`
/// - full device path: `/dev/video2`
/// - udev by-id name: `usb-046d_Webcam_C930e_...`
/// - udev by-path name: `pci-0000:00:14.0-usb-0:2:1.0-video-index0`
///
/// If no argument is given and stdin is a TTY, prompts the user to choose.
/// If no argument is given and stdin is not a TTY, exits with an error
/// (--camera is always required in non-interactive mode).
pub fn resolve_add_camera(explicit: Option<String>) -> u32 {
    let spec = match explicit {
        None => {
            let cameras = list_cameras();
            if cameras.is_empty() {
                eprintln!("faceauth: {}", gettext("no camera devices found"));
                process::exit(1);
            }
            if !stdin_is_tty() {
                // Non-interactive: --camera is always required.
                print_camera_list();
                eprintln!();
                eprintln!(
                    "faceauth: {}",
                    gettext("--camera is required in non-interactive mode")
                );
                process::exit(1);
            }
            // Interactive with multiple cameras (or just one — still ask so
            // the user can confirm the right device is being used).
            return prompt_camera(&cameras);
        }
        Some(s) => s,
    };

    // Integer index.
    if let Ok(n) = spec.parse::<u32>() {
        return n;
    }

    // Full device path: /dev/video2 or /dev/v4l/by-id/... or /dev/v4l/by-path/...
    // Canonicalize to resolve symlinks, then strip the "video" prefix.
    let path = std::path::Path::new(&spec);
    if path.starts_with("/dev") {
        match resolve_video_symlink(path) {
            Some(n) => return n,
            None => {
                eprintln!(
                    "faceauth: {}",
                    gettext("could not resolve camera path '{path}'")
                        .replace("{path}", &spec)
                );
                process::exit(1);
            }
        }
    }

    // udev by-id or by-path name: scan both directories for all entries whose
    // name matches `spec` and collect their resolved indices.
    let mut matches: Vec<(u32, &str)> = Vec::new(); // (index, dir)
    for dir in &["/dev/v4l/by-id", "/dev/v4l/by-path"] {
        let link = std::path::Path::new(dir).join(&spec);
        if let Some(n) = resolve_video_symlink(&link) {
            matches.push((n, dir));
        }
    }

    // Deduplicate: same name in by-id and by-path resolving to the same index is fine.
    matches.dedup_by_key(|(n, _)| *n);

    match matches.as_slice() {
        [] => {}
        [(n, _)] => return *n,
        _ => {
            // Multiple distinct devices share this udev name (e.g. identical
            // cameras without serial numbers). Tell the user to be explicit.
            let indices: Vec<String> = matches
                .iter()
                .map(|(n, _)| format!("/dev/video{}", n))
                .collect();
            eprintln!("faceauth: {}", gettext(
                "'{spec}' is ambiguous - matches: {matches}; use a full path instead (e.g. --camera /dev/video{example})"
            )
            .replace("{spec}", &spec)
            .replace("{matches}", &indices.join(", "))
            .replace("{example}", &matches[0].0.to_string()));
            process::exit(1);
        }
    }

    eprintln!(
        "faceauth: {}",
        gettext(
            "unrecognised camera '{spec}'; use an index, /dev/videoN path, or udev name"
        )
        .replace("{spec}", &spec)
    );
    process::exit(1);
}
