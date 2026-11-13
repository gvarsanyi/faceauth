# TODO

Open items, with the reason each is still open.

## Needs testing on real systems

- polkit face authentication through the socket-activated agent helper
  (stdin peer lookup in the PAM module): verify with a polkit prompt on
  openSUSE (polkit 126+), and with the classic setuid helper elsewhere.
- `faceauth-agent`: verified on the KDE desktop. It now shows no
  notifications while the session is locked and logs each check
  (`journalctl --user -b | grep faceauth-agent`); verify after reinstall.
- GNOME and COSMIC: `faceauth-settings`, the polkit agent, and the lock
  screens (gdm/cosmic-greeter services are not in the default allow-list).
- First GitHub CI run, including the fuzz job and the virtual-camera step
  (vivid, else v4l2loopback via DKMS on the runner kernel).

## Security

- Liveness detection: dlib has no anti-spoofing; needs hardware-specific
  research (IR emitter control, depth). `require_ir` is the current
  mitigation.
- Lock screen re-arming (face counts only after nobody was in view since
  the lock, via logind `LockedHint`): verified on KDE; verify on GNOME
  (with `gdm-password` listed, which is unlock-only).
- COSMIC's locker (cosmic-greeter) does not set logind `LockedHint`, so the
  re-arming does not apply there; propose `SetLockedHint` upstream.
  `cosmic-greeter` is not in the default service list.
- tmux/screen started locally and later attached over SSH, and jobs started
  with `systemd-run --user`, are judged by the local graphical session;
  documented in SECURITY.md, no fix planned.

## Testing

- PAM module integration tests: the module only talks to the fixed system
  bus socket (by design), so a test needs a container with its own system
  bus, the D-Bus policy, the daemon, and pamtester.

## Packaging

- nfpm packages declare no shared-library dependencies (dlib, BLAS/LAPACK,
  libstdc++); nfpm cannot compute them. Some names also differ per distro
  (openSUSE `dbus-1`, `kf6-kirigami-imports`). Declare them per distro, or
  move to native packaging (OBS spec, debian/).
- `libv4l` is probably not needed (no binary links it; V4L2 bindings come
  from kernel headers): verify in the CI containers, then drop it from
  distro-build-deps.md and ci.yml.

## UI consistency (KCM vs GTK)

- KCM: the test timeout (5 s) is hard-coded in C++ and QML and overrides the
  configured `timeout`; its progress bar ignores the back-off wait. Pass 0
  and show an indeterminate bar, like the GTK app.
- KCM: ignores the `EnrollProgress` status ("too-similar" gives no hint);
  `capturesTotal` starts at a hard-coded 5.
- KCM: the Status row ("Service: Available") says little; show the batch
  count instead (GTK does). Delete wording differs ("Delete All" /
  "Clear Face Data?" with OK vs GTK's destructive "Delete").
- KCM: no "IR, recommended" hint in the camera list and no per-service
  descriptions (GTK has both); kcm_faceauth.json Name/Description are not
  translated.
- GTK: controls stay sensitive while a call is pending (a second switch
  toggle starts a concurrent call).
- Both: test errors show the daemon's English message; map `NoMatch` to a
  translated "not recognized" and label other errors differently.
- QML module URI is still `org.faceauth.kcm` (internal only).

## Code quality

- Small duplications: `videoN` index parsing (v4l.rs twice, CLI once), the
  gettext setup and enrollment method timeout (CLI, agent, GTK), the CLI's
  tty check (main.rs, agent.rs); `list_cameras` queries capabilities twice.
- `faceauth add` opens the camera device itself for the IR warning; use the
  suitability from `ListCameras` instead.
- Tests: the daemon's argument parsing, and a check that the daemon's live
  introspection matches `data/dbus/...Manager.xml`.

## Upstream

- Submit `packaging/upstream/pam-config-add-faceauth.patch` to SUSE's
  pam-config (compile and test it first); once released, switch
  `faceauth-setup` to `pam-config --add --faceauth` on openSUSE.
