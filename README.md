# faceauth

https://github.com/gvarsanyi/faceauth

A facial recognition authentication system for Linux, written in Rust. Integrates with PAM (Pluggable Authentication Modules) to enable face-based login as a drop-in replacement or supplement to password authentication.

## Features

- Enroll one or more face encodings per user (supports multiple angles/lighting conditions)
- Authenticate users by comparing live camera feed against stored face models
- PAM module integration for system-level authentication
- CLI for enrollment, testing, and management
- Per-user controls: ignore specific PAM services/applications
- Desktop notifications for authentication events (start, success, failure) via D-Bus
- Secure model storage with atomic writes and strict file permissions
- Fallback support: returns `AUTHINFO_UNAVAIL` on any non-match (no model enrolled, camera unavailable, timeout, etc.), allowing the PAM stack to fall through to password auth
- Automatically skipped for SSH sessions (direct login and any subprocess within, e.g. `sudo`) and background/unattended processes (no controlling terminal); face auth only runs in interactive local sessions

## Architecture

The project is a Cargo workspace with five Rust crates plus a C++/QML KDE component:

| Component | Type | Description |
|---|---|---|
| `faceauth-core` | Rust library | Face detection, encoding, matching, and model persistence |
| `faceauth-cli` | Rust binary | CLI for enrollment, testing, and management |
| `faceauth-daemon` | Rust binary | Privileged daemon that owns camera access, runs authentication, and persists face models |
| `faceauth-pam` | Rust cdylib | PAM module for system authentication |
| `faceauth-notify` | Rust binary | Desktop notification helper spawned by the PAM module |
| `faceauth-kcm` | C++/QML | KDE System Settings module and standalone GUI (`faceauth-gui`) for managing enrollment |

## Requirements

### System Dependencies

See [`distro-build-deps.md`](distro-build-deps.md) for the full per-distro package list. In summary:

- **Runtime**: [dlib](http://dlib.net/) with face recognition support, a V4L2-compatible webcam, Linux PAM
- **Build (Rust components)**: dlib headers, V4L2 headers, libclang (for bindgen), PAM headers
- **Build (KCM/GUI)**: cmake, KF6 (KCMUtils, CoreAddons, Kirigami, I18n), Qt6 (Quick, Network, QuickControls2)
- **Build (icon generation)**: [ImageMagick](https://imagemagick.org/) (`magick` command), needed at `make` time to generate PNG/ICO variants from the SVG master

### Pre-trained Models

The two dlib model files are downloaded automatically during `cargo build`, no
manual steps required:

| File | Size | Source |
|---|---|---|
| `shape_predictor_5_face_landmarks.dat` | ~9.5 MB | [dlib.net](http://dlib.net/files/shape_predictor_5_face_landmarks.dat.bz2) |
| `dlib_face_recognition_resnet_model_v1.dat` | ~21.4 MB | [dlib.net](http://dlib.net/files/dlib_face_recognition_resnet_model_v1.dat.bz2) |

`build.rs` fetches and decompresses them into `faceauth-core/models/` on first
build, then `include_bytes!` embeds them directly into the binary. At runtime
the binary extracts them to `/tmp/faceauth-models-<version>/` on first use
(since dlib requires a file path). A network connection is only needed the
first time you build.

The `.dat` files are listed in `.gitignore` and must not be committed.

## Building and Installation

```bash
make                # build all components (no root required)
sudo make install   # install to system paths and start the daemon
```

`make` builds the Rust workspace and the KCM. `sudo make install` then:
- `target/release/faceauth` -> `/usr/bin/faceauth`
- `target/release/faceauth-daemon` -> `/usr/libexec/faceauth-daemon`
- `target/release/faceauth-notify` -> `/usr/libexec/faceauth-notify`
- `target/release/libpam_faceauth.so` -> `/usr/lib64/security/pam_faceauth.so` (or `/usr/lib/security/` depending on distro)
- KCM plugin -> KDE plugin directory; `faceauth-gui` -> `/usr/bin/faceauth-gui`
- Creates the `faceauthd` system user (if not present)
- Creates `/etc/security/faceauth/` with `750` permissions owned by `faceauthd`
- Installs and starts the `faceauth-daemon` systemd service
- Configures PAM and the KDE lockscreen (see [PAM Configuration](#pam-configuration))

To uninstall: `sudo make uninstall`

> **Note:** The CLI is installed to `/usr/bin` rather than `/usr/local/bin` so it is available on sudo's restricted `PATH`.

### How privilege works

The daemon runs as the `faceauthd` system user and is the only process with access to the camera at authentication time and write access to `/etc/security/faceauth/`. When a user runs `faceauth add` or `faceauth clear`, the CLI:

1. Connects to `/run/faceauth/faceauth.sock`
2. Requests a face capture from the daemon (`CaptureEncoding`), which opens the camera and returns the encoding
3. Sends the encoding (or a clear request) to the daemon (`Enroll` / `Clear`)

The daemon uses `SO_PEERCRED` to obtain the caller's UID from the kernel (this cannot be forged) and only allows a user to modify their own model. Root (UID 0) may manage any user's model.

All CLI commands that modify state (`add`, `clear`) require the daemon and will exit with an error if it is unavailable.

## Usage

```
faceauth [--user USERNAME] COMMAND [OPTIONS]
```

The current user's account is used by default. Root may pass `--user USERNAME`
(or `-u USERNAME`) to manage another user's model.

### Enroll a face

```bash
faceauth add
```

Position your face in front of the camera. The tool captures 5 encodings, prompting you to vary your angle between captures to improve recognition accuracy. Each capture waits up to 30 seconds for a single face to be detected.

### Test authentication

```bash
faceauth test
```

Exits with code `0` on success, `1` on failure. Times out after 5 seconds by default. Prints the camera device being used before attempting authentication.

### Remove a face model

```bash
faceauth clear
```

### Inspect a face model

```bash
faceauth info
```

Prints the enrolled camera's stable udev identifiers and fallback index, and the number of stored encoding batches.

### Ignore specific applications

```bash
faceauth ignore sudo          # never use face auth when sudo asks
faceauth ignore kscreenlocker # skip for the lock screen
faceauth unignore sudo        # remove sudo from the ignore list
```

The ignore list stores PAM service names (e.g. `sudo`, `login`) and parent process names. When either the PAM service or the parent process of the authenticating process matches an entry, face authentication is silently skipped.

### List cameras

```bash
faceauth cameras
```

Lists all detected V4L2 camera devices with their index, name, and a suitability hint (`[IR, recommended]` for greyscale-only devices, `[possible IR]` for devices whose name suggests IR).

### Managing another user's model (root)

```bash
sudo faceauth --user alice add
sudo faceauth --user alice clear
sudo faceauth --user alice info
sudo faceauth --user alice ignore sudo
```

### Options

`add` accepts:
- `--timeout SECONDS` (default 30): time to wait for a face to be detected per capture
- `--camera INDEX`: select the V4L2 device; required when more than one camera is present (`cameras` shows available indices); the camera's stable udev identifiers (`/dev/v4l/by-id/`, `/dev/v4l/by-path/`) are stored in the model and used to reopen the correct device at authentication, even if the device index changes

`test` accepts `--timeout SECONDS` (default 5).

```bash
# Show available cameras and pick the right index
faceauth cameras

# Enroll using /dev/video2 (e.g. an IR camera)
faceauth add --camera 2

# Test: camera is identified from the model automatically
faceauth test
```

## PAM Configuration

`sudo make install` configures PAM automatically on supported distros:

| Distro | Mechanism |
|---|---|
| openSUSE / SLES | Installs `pam-config.d/pam_faceauth` to `/usr/lib/pam-config.d/`, runs `pam-config --add --faceauth` |
| Debian / Ubuntu | Installs `pam-configs/faceauth` descriptor, runs `pam-auth-update --enable faceauth` |
| Other | Prints manual instructions (see below) |

On unsupported distros, add the following line to each PAM service file you want to protect (e.g. `/etc/pam.d/login`, `/etc/pam.d/sudo`) before the `pam_unix` line:

```
auth  sufficient  pam_faceauth.so
auth  required    pam_unix.so
```

The camera used during enrollment is stored in the model file, so no camera configuration is needed in the PAM config.

A successful face match grants access immediately (`sufficient`). Any non-match result (no model enrolled, camera unavailable, timeout, or other error) returns `AUTHINFO_UNAVAIL` and PAM falls through to the next module.

### KDE Plasma lockscreen

KDE Plasma's lockscreen (`kscreenlocker`) runs a second, non-interactive PAM service (`kde-fingerprint`) that runs in a background thread the moment the screen locks. Any module in that service that returns success unlocks the screen immediately, without the user interacting with the password dialog.

`sudo make install` installs `pam.d/kde-fingerprint` (from the repository) to `/etc/pam.d/kde-fingerprint` when KDE is detected (`/etc/pam.d/kde` exists) and no `kde-fingerprint` service is already configured. This gives a hands-free face unlock: the camera activates as soon as the screen locks, and if a face is recognised within the timeout the screen unlocks on its own.

If `kde-fingerprint` already exists (e.g. `fprintd` is configured), the installer leaves it untouched; face auth still works through the main `kde` PAM service via the shared `common-auth` stack, it just requires the password dialog to be active first.

## Desktop Notifications

When face authentication is triggered (e.g. by `sudo` or a lock screen), the PAM module spawns `faceauth-notify` to send standard desktop notifications to the user's graphical session via D-Bus (`org.freedesktop.Notifications`).

| Event | Summary |
|---|---|
| Authentication started | "Face Authentication ..." |
| Authentication succeeded | "Face Authentication Successful" |
| Authentication failed | "Face Authentication Failed" |

The helper is spawned as the authenticated user (dropping privileges via `setuid`/`setgid`) so it can connect to the correct D-Bus session bus at `/run/user/<uid>/bus`. Notification errors are silently ignored; auth outcome is never affected.

## Icon Set

The application icon lives in `assets/icons/faceauth.svg`. PNG variants (16-512 px) and an `.ico` file are generated automatically at build time by `faceauth-notify/build.rs`, which runs `assets/icons/generate.sh` using ImageMagick's `magick` command.

To regenerate manually:

```bash
cd assets/icons
./generate.sh
```

Generated files (`assets/icons/png/` and `assets/icons/*.ico`) are excluded from version control via `.gitignore`.

## How It Works

1. **Enrollment**: The CLI asks the daemon to capture a face encoding (`CaptureEncoding`). The daemon opens the camera, detects a face using dlib, computes a 128-dimensional encoding vector via a ResNet model, and returns it to the CLI. The CLI then sends the encoding to the daemon (`Enroll`), which verifies the caller's identity via `SO_PEERCRED` and appends it to `/etc/security/faceauth/<uid>.json`.

2. **Authentication**: The PAM module (or `faceauth test`) sends an `Authenticate` request to the daemon. The daemon loads the user's stored encodings from disk, opens the enrolled camera, and captures live frames in a loop until a match is found or the timeout expires. A match is declared when the Euclidean distance between a live encoding and a stored encoding is below `0.6` (the threshold recommended by dlib). The daemon reports success or failure back over the socket.

3. **PAM integration**: The PAM module wraps the daemon call with panic catching to prevent C stack corruption, and maps results to appropriate PAM return codes.

## Model Storage

Models are stored as JSON files named by numeric UID (`/etc/security/faceauth/<uid>.json`), with permissions `0640`, owned by the enrolled user. Using the UID means enrollments survive username renames. Writes are atomic (temp file + rename) to prevent corruption.

Each file contains the camera identifiers and the face encoding batches. Legacy fields (`disabled`, `ignore`, `version`) in old model files are silently ignored on load.

## Security Notes

- Model files use `0640` permissions, owned by the enrolled user; readable by the user, not by others
- The PAM module catches panics to safely interop with the C-based PAM stack
- The 0.6 distance threshold is the standard dlib recommendation; tighter thresholds increase security at the cost of false rejections
- Face authentication is refused for: remote sessions (`PAM_RHOST` set: SSH direct login), any process running inside an SSH session (detected by walking the process ancestor chain for `sshd`), and processes with no controlling terminal (cron, systemd services, background scripts); all return `AUTHINFO_UNAVAIL` without contacting the daemon

## License

MIT; see [LICENSE](LICENSE).
