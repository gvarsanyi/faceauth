# Changelog

## 0.10.0

Security release with breaking changes. Enrolled faces and per-service
settings from earlier versions are not carried over; uninstall the old
version and enroll again.

### Security

- The PAM module authenticated the face of `PAM_RUSER` but granted access as
  `PAM_USER`. With `su`, or `sudo` using `targetpw` (openSUSE default), any
  user could become root with their own face. It now only ever matches
  `PAM_USER`.
- Enrollment, deletion, and setting changes were only password-protected in
  the clients. The daemon now enforces polkit authorization and captures
  enrollment images itself; clients can no longer submit face encodings.
- Remote users could switch on the camera (`CaptureEncoding`) and stop face
  authentication for everyone (timeout overflow panic). Camera access now
  requires a local foreground session; timeouts are capped.
- Session detection uses systemd-logind (remote flag, seat, foreground)
  instead of `/dev/tty`, environment variables, and an `sshd` ancestor walk.
- Users could opt every account into any service by injecting newlines into
  a service name. Services are now an administrator allow-list.
- The PAM module ignores `DBUS_SYSTEM_BUS_ADDRESS` and no longer starts
  processes: notifications come from a per-session agent
  (`faceauth-agent`) that the daemon signals.
- Virtual cameras are refused; a missing enrolled camera is an error
  instead of falling back to `/dev/video0`.
- Matching requires `required_matches` consecutive frames with exactly one
  face; the default threshold is 0.45 instead of 0.6.
- Thread-safe user lookups in the daemon; stricter systemd sandbox.

### Changed

- The daemon is a D-Bus system service (`io.github.gvarsanyi.FaceAuth1`),
  D-Bus activated, replacing `/run/faceauth/faceauth.sock`.
- State moved from `/etc/security/faceauth` to `/var/lib/faceauth`.
- New configuration file `/etc/faceauth/faceauth.conf`.
- CLI: `ignore`/`unignore` replaced by `service list|enable|disable`;
  `clear --index` is now `clear --batch` and asks for confirmation; new
  `doctor`; `--json` output; `add --timeout` removed.
- PAM module accepts `timeout=N`.
- KCM uses D-Bus and polkit directly (the KAuth helper is gone).
- Licensed as MIT OR Apache-2.0.
- dlib models are committed and checksum-verified; no download at build
  time. Builds against dlib 19.x and 20.x.
- `make install` supports `PREFIX`/`DESTDIR`; packaging is in the repository;
  man pages added.
- Notifications come from `faceauth-agent`, a per-session agent started by
  XDG autostart (replaces `faceauth-notify`). It shows none while the
  screen is locked and logs every face check to the user journal.
- New GTK 4 / libadwaita app `faceauth-settings` for GNOME, COSMIC, and
  other desktops.
- Face authentication works in polkit's socket-activated agent helper
  (polkit 126+) and in terminals started outside a login session.
- IR cameras with a strobing emitter (dark frames between lit ones) work.
- The daemon unloads the face models after five idle minutes.
- Immediate retry loops (the KDE lock screen) back off: the camera is no
  longer on continuously while the screen is locked and nobody is there.
- Locking while seated no longer unlocks again at once: on a locked session
  (logind `LockedHint`; KDE Plasma, GNOME) a face counts only after nobody
  has been in view for two seconds since the lock.
- Graphical login screens (`gdm-password`, `sddm`, `lightdm`,
  `cosmic-greeter`) only unlock a locked session, never log in.
- `faceauth-setup` and `faceauth doctor` warn when sudo authenticates root
  (`targetpw`/`rootpw`), which makes face authentication inapplicable.
- KDE module translations (`kcm_faceauth` catalog); requires KF6 6.8.
