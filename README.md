# faceauth

Face authentication for Linux. A PAM module asks a small system daemon to
match your face with a webcam, preferably an IR camera, so you can approve
`sudo`, polkit prompts, and the KDE Plasma lock screen by looking at the
camera. Anything other than a match falls through to your password.

> **Security note.** faceauth has no liveness detection. A colour webcam can
> be fooled by a photo; an IR camera is much harder to fool but not
> impossible. Treat face authentication as a convenience, keep a strong
> password, and read [SECURITY.md](SECURITY.md).

## Features

- Daemon-centred design: only `faceauth-daemon` (unprivileged `faceauthd`
  user, sandboxed by systemd) touches the camera and the face data.
- D-Bus API (`io.github.gvarsanyi.FaceAuth1`) with polkit authorization for
  every change and logind checks for every camera use.
- Never used for remote sessions (SSH, remote desktop, including `sudo` inside
  them), background jobs, or sessions that are not in the foreground.
- Administrator decides which PAM services may use face authentication;
  users can opt out per service.
- Requires several consecutive frames with exactly one matching face;
  virtual (frame-injectable) cameras are refused; IR can be made mandatory.
- CLI (`faceauth`), KDE System Settings module and `faceauth-gui`, and a
  GTK/libadwaita app (`faceauth-settings`) for GNOME, COSMIC, and others.
- Desktop notifications while the camera is active, from a per-session
  agent (`faceauth-agent`, XDG autostart).

## Components

| Component | Description |
|---|---|
| `faceauth-common` | D-Bus contract and client, logind checks, config, V4L2 enumeration (no vision code) |
| `faceauth-core` | Face detection and matching (dlib), camera capture, model storage |
| `faceauth-daemon` | System D-Bus service; owns camera and face data |
| `faceauth-pam` | PAM module (`pam_faceauth.so`) |
| `faceauth-cli` | `faceauth` command |
| `faceauth-agent` | Per-session notification agent |
| `faceauth-kcm` | KDE System Settings module and `faceauth-gui` (C++/QML) |
| `faceauth-gtk` | `faceauth-settings`, GTK 4 / libadwaita app |

## Building and installing

Install the [build dependencies](distro-build-deps.md), then:

```bash
make                  # KCM=0 skips the KDE module; the GTK app is built if
                      # its development files are installed (GTK=0/1 forces)
make test
sudo make install     # installs, creates faceauthd, enables the PAM module
faceauth doctor       # verify the setup
faceauth add          # enroll
```

`make install DESTDIR=/path` only stages files (used for packaging).
`make deb` / `make rpm` build packages with nfpm. `sudo make uninstall`
disables the PAM module, deletes all enrolled face data, and removes the
files.

`make install` (and the packages) run `faceauth-setup configure`, which:

- creates the `faceauthd` account (sysusers.d);
- enables the PAM module: on Debian/Ubuntu via `pam-auth-update`; elsewhere
  it adds a marked `auth sufficient pam_faceauth.so` line to each service in
  `services` (copying `/usr/lib/pam.d/<service>` to `/etc/pam.d` if needed).
  Run `/usr/libexec/faceauth-setup configure` again after changing
  `services` (not needed with `pam-auth-update`, where the daemon's
  allow-list decides); `faceauth-setup unconfigure` removes the lines (and
  unchanged copies);
- creates `/etc/pam.d/kde-fingerprint` for hands-free KDE lock-screen unlock
  if Plasma is installed and the slot is unused.

The daemon is D-Bus activated; there is nothing to enable.

## Usage

```bash
faceauth cameras                # list cameras; IR cameras are marked
faceauth add                    # enroll (prompts for the camera the first time)
faceauth add --camera 2         # or choose it explicitly
faceauth test                   # exit status 0 on a match
faceauth info                   # enrolled camera and batches
faceauth service list           # services allowed by the admin, and your choice
faceauth service disable sudo   # stop using face auth for sudo
faceauth clear                  # delete your face data
faceauth doctor                 # diagnose the installation
```

Changing data asks for your password through polkit. With `--user NAME`,
root can read and change other accounts; an administrator can change them
(polkit `manage-other`). Most commands
accept `--json`. See `faceauth(1)`.

## Configuration

`/etc/faceauth/faceauth.conf` (see `faceauth.conf(5)`):

```ini
services = sudo, sudo-i, polkit-1, kde-fingerprint
threshold = 0.45
required_matches = 3
timeout = 5
require_ir = no
```

A service must be listed here and its PAM stack must load
`pam_faceauth.so`. Graphical login screens (`gdm-password`, `sddm`,
`lightdm`, `cosmic-greeter`) only ever unlock an existing, locked session:
logging in by face would leave the keyring, KWallet, and an encrypted home
directory locked. On GNOME, adding `gdm-password` enables face unlock, at
the cost of up to `timeout` seconds before the password prompt when the face
is not recognized; it is not listed by default.

faceauth matches the account being authenticated. If `sudo` asks for the
target's password (`Defaults targetpw`, the openSUSE default) or polkit
treats only root as administrator, that account is root, and face
authentication applies only if root is enrolled. On openSUSE,
`sudo-policy-wheel-auth-self` makes `sudo` authenticate wheel members as
themselves.

PAM module options: `timeout=N` (see `pam_faceauth(8)`). Manual PAM setup:

```
auth  sufficient  pam_faceauth.so
```

before the password module in each service.

## How it works

1. **Enrollment.** `faceauth add` calls `Enroll`; after polkit authorization
   the daemon opens the camera, collects five single-face encodings from
   different angles (128-D dlib descriptors), and stores them in
   `/var/lib/faceauth/<uid>.enc.json` (mode 0600) together with the camera's
   stable udev identifier. Clients never supply encodings.
2. **Authentication.** The PAM module checks the session locally, asks the
   daemon whether the service is allowed and a model exists, and calls
   `Authenticate`. The daemon tells the user's `faceauth-agent`, which shows
   a notification. The daemon succeeds when
   `required_matches` consecutive frames each contain exactly one face within
   `threshold` of an enrolled encoding.
3. **Models.** The dlib models (public domain) are committed under
   `faceauth-core/models/`, verified by SHA-256 at build time, embedded in
   the daemon, and loaded from memory.

## Desktop integration

- **KDE Plasma:** System Settings, Security & Privacy, Face Authentication;
  lock-screen unlock via `kde-fingerprint`.
- **GNOME, COSMIC, others:** `faceauth-settings` (Face Authentication in the
  application menu). PAM integration works wherever the service's stack
  includes the module, for example `polkit-1` for the desktop's polkit
  agent, including polkit's socket-activated helper.

## Development

```bash
cargo build
target/debug/faceauth-daemon --session-bus --state-dir /tmp/fa &
FACEAUTH_BUS=session target/debug/faceauth doctor
```

The session-bus daemon runs as you, without polkit, for testing the CLI,
agent, and GUIs (`FACEAUTH_BUS=session faceauth-gui`,
`FACEAUTH_BUS=session faceauth-settings`). `make GTK=0` and `make KCM=0`
skip the GUIs. Translations: `make pot` regenerates the templates
(`po/faceauth.pot`, `faceauth-kcm/po/kcm_faceauth.pot`) and merges the
catalogs. Fuzzing: `cd fuzz && cargo +nightly fuzz run config` (or
`model_json`). `tests/virtual-camera.sh N` checks that a virtual camera
(`/dev/videoN`, e.g. from `modprobe vivid`) is refused. The D-Bus interface is documented
in `data/dbus/io.github.gvarsanyi.FaceAuth1.Manager.xml`.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The dlib models are public domain
(CC0); vendored crates keep their own licenses (see `vendor/`).
